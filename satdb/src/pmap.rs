//! A persistent (immutable, structurally shared) sorted map: a B+-tree whose nodes are shared through `Arc`.
//!
//! The nodes are wide (up to `B` entries in a leaf, `B` children in a branch), so an update copies about log_B(n) nodes rather than
//! the log_2(n) of a binary tree, and a scan reads contiguous arrays. Old versions stay valid: a node is only ever changed in place
//! when no other version can see it (`Arc::make_mut`, the "transient" idea of Clojure and the reuse idea of Perceus), so a caller that
//! owns a map uniquely (`insert_mut`, `remove_mut`) updates it in place, and one that shares it copies just the path it changes.
//!
//! The branches hold classic B+ separators: `keys.len() == kids.len() - 1`, and every key of `kids[i]` is below `keys[i]`, which is
//! at most every key of `kids[i + 1]`. Deletions never update separators (a stale one still separates), a node that falls below half
//! full is merged with a sibling or rebalanced with it, and every leaf is at the same depth.

use std::sync::Arc;

/// The most entries of a leaf and the most children of a branch (measured: see `index_timings` in the tests of `db`)
const B: usize = 32;
/// The fewest entries (or children) of a node other than the root
const MIN: usize = B / 2;

#[derive(Clone)]
enum Node<K, V>
{
	Leaf
	{
		keys: Vec<K>, vals: Vec<V>
	},
	/// `len` is the number of entries of the subtree, for `rank`
	Branch
	{
		keys: Vec<K>, kids: Vec<Arc<Node<K, V>>>, len: usize
	},
}

impl<K, V> Node<K, V>
{
	fn size(&self) -> usize
	{
		match self
		{
			Node::Leaf { keys, .. } => keys.len(),
			Node::Branch { kids, .. } => kids.len(),
		}
	}

	/// The number of entries of the subtree
	fn count(&self) -> usize
	{
		match self
		{
			Node::Leaf { keys, .. } => keys.len(),
			Node::Branch { len, .. } => *len,
		}
	}

	fn branch(keys: Vec<K>, kids: Vec<Arc<Node<K, V>>>) -> Self
	{
		let len = total(&kids);
		Node::Branch { keys, kids, len }
	}
}

fn total<K, V>(kids: &[Arc<Node<K, V>>]) -> usize
{
	kids.iter().map(|n| n.count()).sum()
}

pub struct PMap<K, V>
{
	root: Option<Arc<Node<K, V>>>,
	len: usize,
}

impl<K, V> Clone for PMap<K, V>
{
	fn clone(&self) -> Self
	{
		PMap { root: self.root.clone(), len: self.len }
	}
}

impl<K, V> Default for PMap<K, V>
{
	fn default() -> Self
	{
		PMap { root: None, len: 0 }
	}
}

/// What a leaf shows of the keys around a position: whether the key before it (at `prev`) or the one at `next` satisfies `same`;
/// None when neither does and one of them is in another leaf (or doesn't exist), so the leaf can't tell
fn adjacent<K>(keys: &[K], prev: Option<usize>, next: usize, same: &dyn Fn(&K) -> bool) -> Option<bool>
{
	match (prev.map(|j| same(&keys[j])), keys.get(next).map(same))
	{
		(Some(true), _) | (_, Some(true)) => Some(true),
		(Some(false), Some(false)) => Some(false),
		_ => None,
	}
}

// Insert into the subtree of `n`; returns the value replaced, and the separator and new right sibling if `n` split.
// With `near`, `found` gets what the leaf shows of the new key's neighbors (see `adjacent`)
fn ins<K: Ord + Clone, V: Clone>(n: &mut Node<K, V>, k: K, v: V, near: Option<&dyn Fn(&K) -> bool>, found: &mut Option<bool>) -> (Option<V>, Option<(K, Arc<Node<K, V>>)>)
{
	match n
	{
		Node::Leaf { keys, vals } => match keys.binary_search(&k)
		{
			Ok(i) => (Some(std::mem::replace(&mut vals[i], v)), None),
			Err(i) =>
			{
				keys.insert(i, k);
				vals.insert(i, v);
				if let Some(same) = near
				{
					*found = adjacent(keys, i.checked_sub(1), i + 1, same);
				}
				if keys.len() <= B
				{
					return (None, None);
				}
				let mid = keys.len() / 2;
				let (rk, rv) = (keys.split_off(mid), vals.split_off(mid));
				(None, Some((rk[0].clone(), Arc::new(Node::Leaf { keys: rk, vals: rv }))))
			}
		},
		Node::Branch { keys, kids, len } =>
		{
			let i = keys.partition_point(|s| *s <= k);
			let (old, split) = ins(Arc::make_mut(&mut kids[i]), k, v, near, found);
			*len += old.is_none() as usize;
			if let Some((sep, right)) = split
			{
				keys.insert(i, sep);
				kids.insert(i + 1, right);
				if kids.len() > B
				{
					// The left half keeps mid children and mid - 1 separators, the next separator goes up
					let mid = kids.len() / 2;
					let rkids = kids.split_off(mid);
					let mut rkeys = keys.split_off(mid - 1);
					let up = rkeys.remove(0);
					*len = total(kids);
					return (old, Some((up, Arc::new(Node::branch(rkeys, rkids)))));
				}
			}
			(old, None)
		}
	}
}

// Remove a present key from the subtree of `n` (which may leave `n` below MIN, for its parent to fix).
// With `near`, `found` gets what the leaf shows of the keys that were around it (see `adjacent`)
fn rem<K: Ord + Clone, V: Clone>(n: &mut Node<K, V>, k: &K, near: Option<&dyn Fn(&K) -> bool>, found: &mut Option<bool>) -> V
{
	match n
	{
		Node::Leaf { keys, vals } =>
		{
			let i = keys.binary_search(k).ok().expect("the key is present");
			keys.remove(i);
			if let Some(same) = near
			{
				*found = adjacent(keys, i.checked_sub(1), i, same);
			}
			vals.remove(i)
		}
		Node::Branch { keys, kids, len } =>
		{
			let i = keys.partition_point(|s| s <= k);
			let old = rem(Arc::make_mut(&mut kids[i]), k, near, found);
			*len -= 1;
			if kids[i].size() < MIN && kids.len() > 1
			{
				rebalance(keys, kids, i);
			}
			old
		}
	}
}

// Fix child i of a branch, below MIN: merge it with a sibling if the two fit in a node, else share their entries evenly
fn rebalance<K: Ord + Clone, V: Clone>(keys: &mut Vec<K>, kids: &mut Vec<Arc<Node<K, V>>>, i: usize)
{
	let l = if i + 1 < kids.len() { i } else { i - 1 };
	let merged = {
		let (a, b) = kids.split_at_mut(l + 1);
		match (Arc::make_mut(&mut a[l]), Arc::make_mut(&mut b[0]))
		{
			(Node::Leaf { keys: lk, vals: lv }, Node::Leaf { keys: rk, vals: rv }) =>
			{
				lk.append(rk);
				lv.append(rv);
				if lk.len() <= B
				{
					true
				}
				else
				{
					let mid = lk.len() / 2;
					*rk = lk.split_off(mid);
					*rv = lv.split_off(mid);
					keys[l] = rk[0].clone();
					false
				}
			}
			(Node::Branch { keys: lk, kids: lc, len: ll }, Node::Branch { keys: rk, kids: rc, len: rl }) =>
			{
				// The separator between the two comes down between their children
				lk.push(keys[l].clone());
				lk.append(rk);
				lc.append(rc);
				*ll += *rl;
				if lc.len() <= B
				{
					true
				}
				else
				{
					let mid = lc.len() / 2;
					*rc = lc.split_off(mid);
					let mut tail = lk.split_off(mid - 1);
					keys[l] = tail.remove(0);
					*rk = tail;
					(*ll, *rl) = (total(lc), total(rc));
					false
				}
			}
			_ => unreachable!("siblings are at the same depth"),
		}
	};
	if merged
	{
		kids.remove(l + 1);
		keys.remove(l);
	}
}

// The sizes of the nodes that hold n items evenly, B at most each (so at least MIN each, when there are two or more)
fn even(n: usize) -> impl Iterator<Item = usize>
{
	let count = n.div_ceil(B).max(1);
	(0..count).map(move |i| n / count + (i < n % count) as usize)
}

type Batch<K, V> = std::iter::Peekable<std::vec::IntoIter<(K, V)>>;

// Insert the batch's entries below `hi` (absent keys, in increasing order) into the subtree of `n` in one pass: a child gets the entries
// below its separator, and one that gets none isn't touched (so not copied). Returns the separators and nodes that follow `n` if it
// overflowed (evenly filled). With `prefix`, pushes for each entry whether a key that was in the map shares its prefix (see `insert_batch_mut`)
fn ins_batch<K: Ord + Clone, V: Clone>(n: &mut Node<K, V>, batch: &mut Batch<K, V>, hi: Option<&K>, prefix: Option<&dyn Fn(&K, &K) -> bool>, out: &mut Vec<Option<bool>>) -> Vec<(K, Arc<Node<K, V>>)>
{
	let below = |k: &K| hi.map_or(true, |h| k < h);
	match n
	{
		Node::Leaf { keys, vals } =>
		{
			// Each entry in place, after the previous one
			let mut new_at = Vec::new();
			let mut i = 0;
			while let Some((k, _)) = batch.peek().filter(|(k, _)| below(k))
			{
				i += keys[i..].partition_point(|o| o < k);
				debug_assert!(keys.get(i) != Some(k), "a batch's keys are absent from the map");
				let (k, v) = batch.next().expect("peeked");
				keys.insert(i, k);
				vals.insert(i, v);
				new_at.push(i);
				i += 1;
			}
			// For each new key, the run of keys with its prefix around it: whether it has an old key, unless it reaches the leaf's edge without one
			if let Some(same) = prefix
			{
				let mut new = vec![false; keys.len()];
				for &j in &new_at
				{
					new[j] = true;
				}
				for &j in &new_at
				{
					let (mut l, mut r) = (j, j + 1);
					while l > 0 && same(&keys[l - 1], &keys[j])
					{
						l -= 1;
					}
					while r < keys.len() && same(&keys[r], &keys[j])
					{
						r += 1;
					}
					out.push(if new[l..r].contains(&false) { Some(true) } else if l == 0 || r == keys.len() { None } else { Some(false) });
				}
			}
			if keys.len() <= B
			{
				return Vec::new();
			}
			// Evenly filled leaves: this one keeps the first part
			let sizes: Vec<usize> = even(keys.len()).collect();
			let mut extra = Vec::with_capacity(sizes.len() - 1);
			for &size in sizes[1..].iter().rev()
			{
				let (k, v) = (keys.split_off(keys.len() - size), vals.split_off(vals.len() - size));
				extra.push((k[0].clone(), Arc::new(Node::Leaf { keys: k, vals: v })));
			}
			extra.reverse();
			extra
		}
		Node::Branch { keys, kids, len } =>
		{
			// Straight to the child of the next entry; it takes every entry below its upper bound
			while let Some((k, _)) = batch.peek().filter(|(k, _)| below(k))
			{
				let i = keys.partition_point(|s| s <= k);
				let kid_hi = if i < keys.len() { Some(keys[i].clone()) } else { hi.cloned() };
				let extra = ins_batch(Arc::make_mut(&mut kids[i]), batch, kid_hi.as_ref(), prefix, out);
				for (j, (sep, node)) in extra.into_iter().enumerate()
				{
					keys.insert(i + j, sep);
					kids.insert(i + 1 + j, node);
				}
			}
			*len = total(kids);
			if kids.len() <= B
			{
				return Vec::new();
			}
			let (new_keys, new_kids) = (std::mem::take(keys), std::mem::take(kids));
			let extra = split_branch(keys, kids, new_keys, new_kids);
			*len = total(kids);
			extra
		}
	}
}

// Fill a branch with these children and separators, evenly split into it and the siblings that follow it (returned) if there are more than B
fn split_branch<K: Clone, V>(keys: &mut Vec<K>, kids: &mut Vec<Arc<Node<K, V>>>, new_keys: Vec<K>, new_kids: Vec<Arc<Node<K, V>>>) -> Vec<(K, Arc<Node<K, V>>)>
{
	let mut out = Vec::new();
	let (mut ks, mut cs) = (new_keys.into_iter(), new_kids.into_iter());
	for (c, size) in even(cs.len()).enumerate()
	{
		// The separator before a chunk goes up; the chunk keeps the size - 1 between its children
		let up = if c > 0 { ks.next() } else { None };
		let (chunk_keys, chunk_kids): (Vec<K>, Vec<Arc<Node<K, V>>>) = (ks.by_ref().take(size - 1).collect(), cs.by_ref().take(size).collect());
		match up
		{
			None => (*keys, *kids) = (chunk_keys, chunk_kids),
			Some(sep) => out.push((sep, Arc::new(Node::branch(chunk_keys, chunk_kids)))),
		}
	}
	out
}

impl<K: Ord + Clone, V: Clone> PMap<K, V>
{
	pub fn new() -> Self
	{
		Self::default()
	}

	pub fn len(&self) -> usize
	{
		self.len
	}

	pub fn is_empty(&self) -> bool
	{
		self.len == 0
	}

	pub fn get(&self, key: &K) -> Option<&V>
	{
		let mut n = self.root.as_deref()?;
		loop
		{
			match n
			{
				Node::Leaf { keys, vals } => return keys.binary_search(key).ok().map(|i| &vals[i]),
				Node::Branch { keys, kids, .. } => n = &kids[keys.partition_point(|s| s <= key)],
			}
		}
	}

	pub fn contains_key(&self, key: &K) -> bool
	{
		self.get(key).is_some()
	}

	/// A new map with `key` bound to `val` (replacing any previous binding)
	pub fn insert(&self, key: K, val: V) -> Self
	{
		let mut m = self.clone();
		m.insert_mut(key, val);
		m
	}

	/// A new map without `key` (the same map, if `key` is absent)
	pub fn remove(&self, key: &K) -> Self
	{
		let mut m = self.clone();
		m.remove_mut(key);
		m
	}

	/// Bind `key` to `val` in this map, in place where no other version shares the nodes; returns the value replaced
	pub fn insert_mut(&mut self, key: K, val: V) -> Option<V>
	{
		self.ins_root(key, val, None).0
	}

	/// `insert_mut` of an absent key, which also tells whether one of its neighbors in key order satisfies `same` (a monotone-range
	/// predicate, such as sharing the key's prefix, so that the answer is whether any other key does), when the leaf shows it:
	/// None when the answer depends on a key in another leaf. It saves a separate search for "is there another key with this prefix?"
	pub fn insert_mut_near(&mut self, key: K, val: V, same: impl Fn(&K) -> bool) -> Option<bool>
	{
		let (old, found) = self.ins_root(key, val, Some(&same));
		debug_assert!(old.is_none(), "the key was absent");
		found
	}

	fn ins_root(&mut self, key: K, val: V, near: Option<&dyn Fn(&K) -> bool>) -> (Option<V>, Option<bool>)
	{
		let Some(root) = &mut self.root
		else
		{
			self.root = Some(Arc::new(Node::Leaf { keys: vec![key], vals: vec![val] }));
			self.len = 1;
			return (None, Some(false));
		};
		let mut found = None;
		let (old, split) = ins(Arc::make_mut(root), key, val, near, &mut found);
		if let Some((sep, right)) = split
		{
			let left = self.root.take().expect("the root just split");
			self.root = Some(Arc::new(Node::branch(vec![sep], vec![left, right])));
		}
		self.len += old.is_none() as usize;
		(old, found)
	}

	/// Remove `key` from this map, in place where no other version shares the nodes; returns its value (an absent key copies nothing)
	pub fn remove_mut(&mut self, key: &K) -> Option<V>
	{
		self.rem_root(key, None).map(|r| r.0)
	}

	/// `remove_mut`, which also tells whether one of the keys that were around `key` satisfies `same` (see `insert_mut_near`);
	/// None for an absent key
	pub fn remove_mut_near(&mut self, key: &K, same: impl Fn(&K) -> bool) -> Option<(V, Option<bool>)>
	{
		self.rem_root(key, Some(&same))
	}

	fn rem_root(&mut self, key: &K, near: Option<&dyn Fn(&K) -> bool>) -> Option<(V, Option<bool>)>
	{
		if !self.contains_key(key)
		{
			return None;
		}
		let root = self.root.as_mut().expect("the key is present");
		let mut found = None;
		let old = rem(Arc::make_mut(root), key, near, &mut found);
		self.len -= 1;
		// A root branch with one child gives way to it, and an empty root leaf to the empty map
		match self.root.as_deref()
		{
			Some(Node::Leaf { keys, .. }) if keys.is_empty() => self.root = None,
			Some(Node::Branch { kids, .. }) if kids.len() == 1 => self.root = Some(kids[0].clone()),
			_ => {}
		}
		Some((old, found))
	}

	/// Insert `entries` (keys absent from the map, strictly increasing) in one pass, in place where no other version shares the nodes:
	/// each node that gets entries is visited (and, if shared, copied) once, rather than once per entry, so the entries that land near
	/// each other (one entity's datoms, one transaction's log) share their descents.
	/// With `prefix` (whether two keys share a prefix, which must group contiguous keys), returns for each entry whether a key that was in
	/// the map before shares its prefix, when its leaf shows it (None: the run of keys with its prefix reaches the leaf's edge without one)
	pub fn insert_batch_mut(&mut self, entries: Vec<(K, V)>, prefix: Option<&dyn Fn(&K, &K) -> bool>) -> Vec<Option<bool>>
	{
		debug_assert!(entries.windows(2).all(|w| w[0].0 < w[1].0), "the keys must be strictly increasing");
		let count = entries.len();
		let Some(root) = &mut self.root
		else
		{
			*self = Self::from_sorted(entries);
			return if prefix.is_some() { vec![Some(false); count] } else { Vec::new() };
		};
		let mut out = Vec::with_capacity(if prefix.is_some() { count } else { 0 });
		let mut batch = entries.into_iter().peekable();
		let mut extra = ins_batch(Arc::make_mut(root), &mut batch, None, prefix, &mut out);
		debug_assert!(batch.peek().is_none(), "the root takes every entry");
		// The root overflowed: new levels above it, until one node holds the level
		while !extra.is_empty()
		{
			let first = self.root.take().expect("the root");
			let (mut keys, mut kids) = (Vec::new(), Vec::new());
			let mut new_keys = Vec::with_capacity(extra.len());
			let mut new_kids = vec![first];
			for (sep, node) in extra
			{
				new_keys.push(sep);
				new_kids.push(node);
			}
			extra = split_branch(&mut keys, &mut kids, new_keys, new_kids);
			self.root = Some(Arc::new(Node::branch(keys, kids)));
		}
		self.len += count;
		out
	}

	/// The map of `entries`, whose keys must be strictly increasing, built in linear time with every node filled evenly
	/// (rather than by inserting them one by one)
	pub fn from_sorted(entries: impl IntoIterator<Item = (K, V)>) -> Self
	{
		let (keys, vals): (Vec<K>, Vec<V>) = entries.into_iter().unzip();
		debug_assert!(keys.windows(2).all(|w| w[0] < w[1]), "the keys must be strictly increasing");
		let len = keys.len();
		if len == 0
		{
			return Self::default();
		}
		// Each level as (first key, node)
		let (mut ks, mut vs) = (keys.into_iter(), vals.into_iter());
		let mut level: Vec<(K, Arc<Node<K, V>>)> = even(len)
			.map(|s| {
				let (lk, lv): (Vec<K>, Vec<V>) = (ks.by_ref().take(s).collect(), vs.by_ref().take(s).collect());
				(lk[0].clone(), Arc::new(Node::Leaf { keys: lk, vals: lv }))
			})
			.collect();
		while level.len() > 1
		{
			let mut items = level.into_iter();
			level = even(items.len())
				.map(|s| {
					let group: Vec<(K, Arc<Node<K, V>>)> = items.by_ref().take(s).collect();
					let first = group[0].0.clone();
					let (firsts, kids): (Vec<K>, Vec<Arc<Node<K, V>>>) = group.into_iter().unzip();
					(first, Arc::new(Node::branch(firsts.into_iter().skip(1).collect(), kids)))
				})
				.collect();
		}
		PMap { root: level.pop().map(|(_, n)| n), len }
	}

	/// All the entries in ascending key order
	pub fn iter(&self) -> Iter<'_, K, V>
	{
		self.seek(|_| false)
	}

	/// The entries in ascending key order, starting at the first key for which `below` is false.
	/// `below` must be monotone (true for a prefix of the key order), e.g., `|k| k < &lower_bound`,
	/// which lets a caller seek to the start of a key prefix without constructing a minimal key.
	pub fn seek(&self, below: impl Fn(&K) -> bool) -> Iter<'_, K, V>
	{
		let mut it = Iter { stack: Vec::new(), keys: &[], vals: &[], pos: 0 };
		let Some(mut n) = self.root.as_deref() else { return it };
		loop
		{
			match n
			{
				// The children before j hold only keys below a separator that is below; the first key not below is in kids[j] or after it
				Node::Branch { keys, kids, .. } =>
				{
					let j = keys.partition_point(|s| below(s));
					it.stack.push((kids, j));
					n = &kids[j];
				}
				Node::Leaf { keys, vals } =>
				{
					(it.keys, it.vals, it.pos) = (keys, vals, keys.partition_point(|k| below(k)));
					return it;
				}
			}
		}
	}

	/// Check the structure (sorted keys, separators that separate, every node but the root at least half full, every leaf at one depth)
	#[cfg(test)]
	pub(crate) fn check(&self)
	{
		// Returns the depth and the number of entries of the subtree, checking that its keys are within [lo, hi)
		fn walk<K: Ord, V>(n: &Node<K, V>, root: bool, lo: Option<&K>, hi: Option<&K>) -> (usize, usize)
		{
			assert!(root || n.size() >= MIN, "a node below half full");
			assert!(n.size() <= B, "an overfull node");
			match n
			{
				Node::Leaf { keys, vals } =>
				{
					assert_eq!(keys.len(), vals.len());
					assert!(keys.windows(2).all(|w| w[0] < w[1]));
					assert!(keys.iter().all(|k| lo.map_or(true, |l| l <= k) && hi.map_or(true, |h| k < h)), "a key outside its separators");
					(1, keys.len())
				}
				Node::Branch { keys, kids, len } =>
				{
					assert_eq!(keys.len() + 1, kids.len());
					assert!(kids.len() >= 2, "a branch with one child");
					assert!(keys.windows(2).all(|w| w[0] < w[1]));
					let mut depth = None;
					let mut count = 0;
					for (i, kid) in kids.iter().enumerate()
					{
						let (l, h) = (if i == 0 { lo } else { Some(&keys[i - 1]) }, if i == keys.len() { hi } else { Some(&keys[i]) });
						let (d, c) = walk(kid, false, l, h);
						assert!(depth.map_or(true, |x| x == d), "leaves at different depths");
						depth = Some(d);
						count += c;
					}
					assert_eq!(*len, count, "a branch's count");
					(1 + depth.expect("a branch has children"), count)
				}
			}
		}
		match &self.root
		{
			None => assert_eq!(self.len, 0),
			Some(r) =>
			{
				assert!(r.size() > 0, "an empty root");
				assert_eq!(walk(r, true, None, None).1, self.len);
			}
		}
	}

	/// The number of keys for which `below` is true (monotone, as for `seek`: the keys before the one `seek` starts at),
	/// in logarithmic time, from the subtrees' counts
	pub fn rank(&self, below: impl Fn(&K) -> bool) -> usize
	{
		let mut rank = 0;
		let Some(mut n) = self.root.as_deref() else { return 0 };
		loop
		{
			match n
			{
				// As in seek: the children before j hold only keys that are below
				Node::Branch { keys, kids, .. } =>
				{
					let j = keys.partition_point(|s| below(s));
					rank += total(&kids[..j]);
					n = &kids[j];
				}
				Node::Leaf { keys, .. } => return rank + keys.partition_point(|k| below(k)),
			}
		}
	}
}

pub struct Iter<'a, K, V>
{
	/// The branches on the path to the current leaf, each with the index of the child on the path
	stack: Vec<(&'a [Arc<Node<K, V>>], usize)>,
	keys: &'a [K],
	vals: &'a [V],
	pos: usize,
}

impl<'a, K, V> Iterator for Iter<'a, K, V>
{
	type Item = (&'a K, &'a V);

	fn next(&mut self) -> Option<Self::Item>
	{
		while self.pos == self.keys.len()
		{
			// The next leaf: up to the first branch with a child after the path's, then down its leftmost path
			let mut n: &'a Node<K, V> = loop
			{
				let (kids, i) = self.stack.last_mut()?;
				if *i + 1 < kids.len()
				{
					*i += 1;
					let kids: &'a [Arc<Node<K, V>>] = kids;
					break &kids[*i];
				}
				self.stack.pop();
			};
			loop
			{
				match n
				{
					Node::Branch { kids, .. } =>
					{
						self.stack.push((kids, 0));
						n = &kids[0];
					}
					Node::Leaf { keys, vals } =>
					{
						(self.keys, self.vals, self.pos) = (keys, vals, 0);
						break;
					}
				}
			}
		}
		self.pos += 1;
		Some((&self.keys[self.pos - 1], &self.vals[self.pos - 1]))
	}
}

#[cfg(test)]
mod tests
{
	use super::*;
	use crate::testutil::Rng;
	use std::collections::BTreeMap;

	// Random insertions and removals, by both the persistent and the in-place operations, against BTreeMap; every version is kept,
	// checked after every operation, and compared again at the end (so an in-place change to a shared node would show)
	#[test]
	fn matches_btreemap_and_keeps_old_versions()
	{
		for (seed, range) in [(1, 500), (2, 40), (3, 5000)]
		{
			let mut rng = Rng::new(seed);
			let mut model = BTreeMap::new();
			let mut map = PMap::new();
			let mut versions = Vec::new();
			for step in 0..4000
			{
				let k = rng.next() % range;
				let in_place = rng.next() % 2 == 0;
				// Long runs of removals, to empty the map and shrink the tree
				if rng.next() % 3 == 0 || (step / 500) % 4 == 3
				{
					assert_eq!(model.remove(&k), if in_place { map.remove_mut(&k) } else { map.get(&k).copied() });
					if !in_place
					{
						map = map.remove(&k);
					}
				}
				else
				{
					let v = rng.next();
					assert_eq!(model.insert(k, v), if in_place { map.insert_mut(k, v) } else { map.get(&k).copied() });
					if !in_place
					{
						map = map.insert(k, v);
					}
				}
				map.check();
				versions.push((map.clone(), model.clone()));
			}
			for (m, b) in &versions
			{
				m.check();
				assert_eq!(m.len(), b.len());
				assert!(m.iter().map(|(k, v)| (*k, *v)).eq(b.iter().map(|(k, v)| (*k, *v))));
				let lo = range / 2;
				assert!(m.seek(|k| *k < lo).map(|(k, _)| *k).eq(b.range(lo..).map(|(k, _)| *k)));
				assert_eq!(m.get(&7), b.get(&7));
				for bound in [0, 1, range / 2, range - 1, range, range + 100]
				{
					assert_eq!(m.rank(|k| *k < bound), b.range(..bound).count(), "the rank of {bound}");
				}
			}
		}
	}

	// seek with bounds on every key and between keys (separators included), and with an always-true and an always-false predicate
	#[test]
	fn seek_every_bound()
	{
		let model: BTreeMap<u64, u64> = (0..2000u64).map(|k| (k * 3, k)).collect();
		let map = PMap::from_sorted(model.iter().map(|(k, v)| (*k, *v)));
		for lo in 0..6010u64
		{
			assert!(map.seek(|k| *k < lo).map(|(k, _)| *k).eq(model.range(lo..).map(|(k, _)| *k)), "lo {lo}");
		}
		assert_eq!(map.seek(|_| true).next(), None);
		assert_eq!(map.seek(|_| false).count(), 2000);
	}

	// A bulk-built map has the entries and the structure of one built by insertions, at every size, and keeps working as a map
	#[test]
	fn from_sorted_matches_insertions()
	{
		let mut rng = Rng::new(9);
		for n in [0, 1, 2, B - 1, B, B + 1, 2 * B, B * B, B * B + 1, 7 * B * B + 3, 3000]
		{
			let mut model = BTreeMap::new();
			while model.len() < n
			{
				model.insert(rng.next() % 100_000, rng.next());
			}
			let bulk = PMap::from_sorted(model.iter().map(|(k, v)| (*k, *v)));
			bulk.check();
			assert_eq!(bulk.len(), n);
			assert!(bulk.iter().map(|(k, v)| (*k, *v)).eq(model.iter().map(|(k, v)| (*k, *v))));
			let (mut m, mut b) = (bulk, model);
			for _ in 0..200
			{
				let k = rng.next() % 100_000;
				if rng.next() % 2 == 0
				{
					m.remove_mut(&k);
					b.remove(&k);
				}
				else
				{
					m.insert_mut(k, 1);
					b.insert(k, 1);
				}
				m.check();
			}
			assert!(m.iter().map(|(k, v)| (*k, *v)).eq(b.iter().map(|(k, v)| (*k, *v))));
			for bound in [0, 17, 5_000, 9_999, 10_000]
			{
				assert_eq!(m.rank(|k| *k < bound), b.range(..bound).count(), "the rank of {bound} among {n} and more");
			}
		}
	}

	// What insert_mut_near and remove_mut_near tell about other keys with the same prefix agrees with a scan whenever they tell
	#[test]
	fn near_answers_agree_with_a_scan()
	{
		let mut rng = Rng::new(21);
		let mut map: PMap<(u64, u64), ()> = PMap::new();
		let (mut told, mut untold) = (0, 0);
		for _ in 0..20_000
		{
			let k = (rng.next() % 300, rng.next() % 8);
			let same = |x: &(u64, u64)| x.0 == k.0;
			let others = |m: &PMap<(u64, u64), ()>| m.iter().filter(|(x, _)| x.0 == k.0 && **x != k).count() > 0;
			let answer = if map.contains_key(&k) { map.remove_mut_near(&k, same).unwrap().1 } else { map.insert_mut_near(k, (), same) };
			match answer
			{
				Some(a) =>
				{
					assert_eq!(a, others(&map), "{k:?}");
					told += 1;
				}
				None => untold += 1,
			}
		}
		map.check();
		assert!(told > 10 * untold, "the leaf should usually tell ({told} told, {untold} not)");
	}

	// Batches of new keys (small and large, clustered and scattered) against BTreeMap, keeping every version, with the prefix answers
	// checked against the map before the batch
	#[test]
	fn batches_match_btreemap()
	{
		let mut rng = Rng::new(31);
		let mut model: BTreeMap<(u64, u64), u64> = BTreeMap::new();
		let mut map: PMap<(u64, u64), u64> = PMap::new();
		let mut versions = Vec::new();
		let (mut told, mut untold) = (0, 0);
		for round in 0..300
		{
			let size = [1, 2, 5, 20, 40, 200][round % 6];
			let mut batch = BTreeMap::new();
			while batch.len() < size
			{
				// Clustered on one prefix, or scattered
				let k = if round % 3 == 0 { (round as u64 % 50, rng.next() % 1000) } else { (rng.next() % 400, rng.next() % 20) };
				if !model.contains_key(&k)
				{
					batch.insert(k, rng.next());
				}
			}
			let before = model.clone();
			let answers = map.insert_batch_mut(batch.iter().map(|(k, v)| (*k, *v)).collect(), Some(&|a: &(u64, u64), b: &(u64, u64)| a.0 == b.0));
			assert_eq!(answers.len(), batch.len());
			for ((k, _), a) in batch.iter().zip(&answers)
			{
				match a
				{
					Some(a) =>
					{
						assert_eq!(*a, before.keys().any(|x| x.0 == k.0), "{k:?}");
						told += 1;
					}
					None => untold += 1,
				}
			}
			model.extend(batch);
			map.check();
			assert!(map.iter().map(|(k, v)| (*k, *v)).eq(model.iter().map(|(k, v)| (*k, *v))));
			// Removals too, so that the tree shrinks between batches
			for _ in 0..size / 2
			{
				let k = *model.keys().nth((rng.next() % model.len() as u64) as usize).unwrap();
				model.remove(&k);
				map.remove_mut(&k);
			}
			map.check();
			versions.push((map.clone(), model.clone()));
		}
		for (m, b) in &versions
		{
			m.check();
			assert!(m.iter().map(|(k, v)| (*k, *v)).eq(b.iter().map(|(k, v)| (*k, *v))));
		}
		assert!(told > 5 * untold, "{told} told, {untold} not");
		// Into an empty map, and a batch that grows the tree by several levels at once
		let mut m: PMap<u64, u64> = PMap::new();
		m.insert_batch_mut(vec![(1, 1), (2, 2)], None);
		m.insert_batch_mut((3..40_000).map(|k| (k, k)).collect(), None);
		m.check();
		assert!(m.iter().map(|(k, _)| *k).eq(1..40_000));
	}

	// An in-place update of a map copies the nodes it shares with another version, and only those
	#[test]
	fn in_place_updates_copy_shared_nodes_only()
	{
		let mut a = PMap::from_sorted((0..10_000u64).map(|k| (k, k)));
		let b = a.clone();
		a.insert_mut(5, 0);
		assert_eq!((a.get(&5), b.get(&5)), (Some(&0), Some(&5)));
		// The path to key 5 was copied, the rest is shared
		let (Some(ra), Some(rb)) = (&a.root, &b.root) else { unreachable!() };
		let (Node::Branch { kids: ka, .. }, Node::Branch { kids: kb, .. }) = (&**ra, &**rb) else { unreachable!() };
		assert!(!Arc::ptr_eq(&ka[0], &kb[0]) && Arc::ptr_eq(ka.last().unwrap(), kb.last().unwrap()));
		// Uniquely owned now, the path is updated in place
		let before = Arc::as_ptr(&a.root.as_ref().unwrap());
		a.insert_mut(6, 0);
		a.remove_mut(&7);
		assert_eq!(Arc::as_ptr(&a.root.as_ref().unwrap()), before);
		assert_eq!((b.get(&6), b.get(&7)), (Some(&6), Some(&7)));
	}
}
