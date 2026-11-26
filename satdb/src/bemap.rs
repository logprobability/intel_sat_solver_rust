//! A persistent Bε-tree (Brodal and Fagerberg 2003; the write-optimized trees of Bender, Farach-Colton et al.), to measure against the
//! B+-tree of `pmap`: the same nodes, plus a buffer of pending messages (a put or a delete per key) in every branch.
//!
//! An update is a message into the root's buffer; a buffer over `CAP` is flushed one level down in one sorted pass, each child getting its
//! run of messages (into its buffer, flushed in turn when full, or applied to a leaf), so a leaf is touched once per batch of messages
//! rather than once per update. (Flushing only the child with the most messages, the textbook policy, measured 3x slower than the B+-tree
//! in memory: it rescans the buffer for each child's few messages.)
//!
//! **Not used by `Db`.** Measured against `PMap` (`cargo test --release bemap_vs_pmap -- --ignored --nocapture`: 1M scattered insertions,
//! 1M point reads, a full scan; three runs of two rounds on a noisy machine): with `CAP` = 256 (128, 512, 1024 and 2048 measured worse),
//! insertions are 0.91-1.85x as fast (about 1.35x typically), point reads 1.44-1.79x slower and scans 2.3-5.3x slower. A transaction reads
//! the indexes as it plans and queries scan them, and the statistics need neighbor answers that a buffered insertion can't give, so the
//! B+-tree stays. In memory, an insertion into the B+-tree already costs only its descent's cache misses and a short shift within a leaf,
//! which is what the buffers would amortize; the Bε-tree's advantage is in block transfers, which an in-memory index doesn't make.
//! Newer messages are higher: a lookup takes the first message on its path (a put gives the value, a delete nothing), and an iteration
//! merges each leaf with the messages its ancestors hold for the leaf's key range. A buffered update can't tell whether its key was there,
//! so the map keeps its length by contract: `insert_new` takes an absent key, `remove_present` a present one.
//! Nodes are shared through `Arc` and changed in place when unshared (`Arc::make_mut`), as in `pmap`.

use std::sync::Arc;

const B: usize = 32;
const MIN: usize = B / 2;
/// The most messages a branch holds before it flushes
const CAP: usize = 256;

#[derive(Clone)]
enum Msg<V>
{
	Put(V),
	Del,
}

#[derive(Clone)]
enum Node<K, V>
{
	Leaf
	{
		keys: Vec<K>, vals: Vec<V>
	},
	Branch
	{
		keys: Vec<K>, kids: Vec<Arc<Node<K, V>>>, buf: Vec<(K, Msg<V>)>
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
}

pub struct BeMap<K, V>
{
	root: Option<Arc<Node<K, V>>>,
	len: usize,
}

impl<K, V> Clone for BeMap<K, V>
{
	fn clone(&self) -> Self
	{
		BeMap { root: self.root.clone(), len: self.len }
	}
}

impl<K, V> Default for BeMap<K, V>
{
	fn default() -> Self
	{
		BeMap { root: None, len: 0 }
	}
}

fn even(n: usize) -> impl Iterator<Item = usize>
{
	let count = n.div_ceil(B).max(1);
	(0..count).map(move |i| n / count + (i < n % count) as usize)
}

// Merge sorted messages into a buffer, the new ones replacing older ones of the same key
fn merge_msgs<K: Ord, V>(buf: &mut Vec<(K, Msg<V>)>, msgs: Vec<(K, Msg<V>)>)
{
	if buf.is_empty()
	{
		*buf = msgs;
		return;
	}
	let old = std::mem::take(buf);
	let mut a = old.into_iter().peekable();
	for (k, m) in msgs
	{
		while a.peek().is_some_and(|(o, _)| *o < k)
		{
			buf.push(a.next().expect("peeked"));
		}
		if a.peek().is_some_and(|(o, _)| *o == k)
		{
			a.next();
		}
		buf.push((k, m));
	}
	buf.extend(a);
}

// Apply sorted messages to a leaf; returns the leaves that follow it if it overflowed (evenly filled)
fn apply_leaf<K: Ord + Clone, V: Clone>(keys: &mut Vec<K>, vals: &mut Vec<V>, msgs: Vec<(K, Msg<V>)>) -> Vec<(K, Arc<Node<K, V>>)>
{
	let mut i = 0;
	for (k, m) in msgs
	{
		i += keys[i..].partition_point(|o| *o < k);
		let present = keys.get(i) == Some(&k);
		match (m, present)
		{
			(Msg::Put(v), true) => vals[i] = v,
			(Msg::Put(v), false) =>
			{
				keys.insert(i, k);
				vals.insert(i, v);
			}
			(Msg::Del, true) =>
			{
				keys.remove(i);
				vals.remove(i);
			}
			(Msg::Del, false) => {}
		}
	}
	if keys.len() <= B
	{
		return Vec::new();
	}
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

// Split an overfull branch evenly, its buffer following the key ranges; returns the branches that follow it
fn split_branch<K: Ord + Clone, V: Clone>(keys: &mut Vec<K>, kids: &mut Vec<Arc<Node<K, V>>>, buf: &mut Vec<(K, Msg<V>)>) -> Vec<(K, Arc<Node<K, V>>)>
{
	if kids.len() <= B
	{
		return Vec::new();
	}
	let (all_keys, all_kids, all_buf) = (std::mem::take(keys), std::mem::take(kids), std::mem::take(buf));
	let (mut ks, mut cs, mut ms) = (all_keys.into_iter(), all_kids.into_iter(), all_buf.into_iter().peekable());
	let mut out = Vec::new();
	let sizes: Vec<usize> = even(cs.len()).collect();
	for (c, &size) in sizes.iter().enumerate()
	{
		let up = if c > 0 { ks.next() } else { None };
		let chunk_keys: Vec<K> = ks.by_ref().take(size - 1).collect();
		let chunk_kids: Vec<Arc<Node<K, V>>> = cs.by_ref().take(size).collect();
		// The messages below the next chunk's separator (the one after this chunk's keys)
		let next_sep = ks.as_slice().first().cloned();
		let mut chunk_buf = Vec::new();
		while let Some((k, _)) = ms.peek()
		{
			if next_sep.as_ref().is_some_and(|s| k >= s) && c + 1 < sizes.len()
			{
				break;
			}
			chunk_buf.push(ms.next().expect("peeked"));
		}
		match up
		{
			None => (*keys, *kids, *buf) = (chunk_keys, chunk_kids, chunk_buf),
			Some(sep) => out.push((sep, Arc::new(Node::Branch { keys: chunk_keys, kids: chunk_kids, buf: chunk_buf }))),
		}
	}
	out
}

// Fix child i of a branch, below MIN: merge it with a sibling if the two fit, else share their entries (and buffers) evenly
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
			(Node::Branch { keys: lk, kids: lc, buf: lb }, Node::Branch { keys: rk, kids: rc, buf: rb }) =>
			{
				lk.push(keys[l].clone());
				lk.append(rk);
				lc.append(rc);
				lb.append(rb);
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
					let at = lb.partition_point(|(k, _)| *k < keys[l]);
					*rb = lb.split_off(at);
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

// Flush a full branch buffer in one sorted pass: each child gets its run of messages (into its buffer, flushed in turn if full, or applied
// to a leaf), and the children left below half full are fixed afterwards. (In memory, emptying the whole buffer at once costs one pass per
// CAP messages, where flushing the fullest child each time rescans the buffer for every child's few messages.)
// Returns the branches that follow this one if it overflowed
fn flush<K: Ord + Clone, V: Clone>(keys: &mut Vec<K>, kids: &mut Vec<Arc<Node<K, V>>>, buf: &mut Vec<(K, Msg<V>)>) -> Vec<(K, Arc<Node<K, V>>)>
{
	if buf.len() > CAP
	{
		let mut msgs = std::mem::take(buf).into_iter().peekable();
		while let Some((k, _)) = msgs.peek()
		{
			let i = keys.partition_point(|s| s <= k);
			let mut run = Vec::new();
			while let Some((k, _)) = msgs.peek()
			{
				if i < keys.len() && *k >= keys[i]
				{
					break;
				}
				run.push(msgs.next().expect("peeked"));
			}
			let extra = match Arc::make_mut(&mut kids[i])
			{
				Node::Leaf { keys: lk, vals: lv } => apply_leaf(lk, lv, run),
				Node::Branch { keys: ck, kids: cc, buf: cb } =>
				{
					merge_msgs(cb, run);
					flush(ck, cc, cb)
				}
			};
			for (j, (sep, node)) in extra.into_iter().enumerate()
			{
				keys.insert(i + j, sep);
				kids.insert(i + 1 + j, node);
			}
		}
		// Underfull children, from the right (a merge keeps the indices to its left)
		let mut i = kids.len();
		while i > 0
		{
			i -= 1;
			if i < kids.len() && kids.len() > 1 && kids[i].size() < MIN
			{
				rebalance(keys, kids, i);
			}
		}
	}
	split_branch(keys, kids, buf)
}

impl<K: Ord + Clone, V: Clone> BeMap<K, V>
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
				Node::Branch { keys, kids, buf } =>
				{
					if let Ok(j) = buf.binary_search_by(|(k, _)| k.cmp(key))
					{
						return match &buf[j].1
						{
							Msg::Put(v) => Some(v),
							Msg::Del => None,
						};
					}
					n = &kids[keys.partition_point(|s| s <= key)];
				}
			}
		}
	}

	pub fn contains_key(&self, key: &K) -> bool
	{
		self.get(key).is_some()
	}

	// Send a message: into the root's buffer (flushing it when full), or straight to a root leaf
	fn send(&mut self, key: K, msg: Msg<V>)
	{
		let Some(root) = &mut self.root
		else
		{
			if let Msg::Put(v) = msg
			{
				self.root = Some(Arc::new(Node::Leaf { keys: vec![key], vals: vec![v] }));
			}
			return;
		};
		let extra = match Arc::make_mut(root)
		{
			Node::Leaf { keys, vals } => apply_leaf(keys, vals, vec![(key, msg)]),
			Node::Branch { keys, kids, buf } =>
			{
				match buf.binary_search_by(|(k, _)| k.cmp(&key))
				{
					Ok(j) => buf[j].1 = msg,
					Err(j) => buf.insert(j, (key, msg)),
				}
				flush(keys, kids, buf)
			}
		};
		self.grow(extra);
		// A root branch with one child gives way to it (its messages go into the child first), and an empty root leaf to the empty map
		loop
		{
			match self.root.as_deref()
			{
				Some(Node::Leaf { keys, .. }) if keys.is_empty() => self.root = None,
				Some(Node::Branch { kids, .. }) if kids.len() == 1 =>
				{
					let Some(Node::Branch { kids, buf, .. }) = self.root.as_mut().map(Arc::make_mut) else { unreachable!() };
					let (mut kid, msgs) = (kids.pop().expect("one child"), std::mem::take(buf));
					let extra = match Arc::make_mut(&mut kid)
					{
						Node::Leaf { keys, vals } => apply_leaf(keys, vals, msgs),
						Node::Branch { keys, kids, buf } =>
						{
							merge_msgs(buf, msgs);
							flush(keys, kids, buf)
						}
					};
					self.root = Some(kid);
					self.grow(extra);
					continue;
				}
				_ => {}
			}
			break;
		}
	}

	// The root split: new levels above it, until one node holds the level
	fn grow(&mut self, mut extra: Vec<(K, Arc<Node<K, V>>)>)
	{
		while !extra.is_empty()
		{
			let first = self.root.take().expect("the root");
			let (mut keys, mut kids, mut buf) = (Vec::new(), vec![first], Vec::new());
			for (sep, node) in extra
			{
				keys.push(sep);
				kids.push(node);
			}
			extra = split_branch(&mut keys, &mut kids, &mut buf);
			self.root = Some(Arc::new(Node::Branch { keys, kids, buf }));
		}
	}

	/// Bind an absent key (by contract: the map can't check without a search, which is what the buffer saves)
	pub fn insert_new(&mut self, key: K, val: V)
	{
		self.send(key, Msg::Put(val));
		self.len += 1;
	}

	/// Remove a present key (by contract, as `insert_new`)
	pub fn remove_present(&mut self, key: &K)
	{
		self.send(key.clone(), Msg::Del);
		self.len -= 1;
	}

	pub fn from_sorted(entries: impl IntoIterator<Item = (K, V)>) -> Self
	{
		let (keys, vals): (Vec<K>, Vec<V>) = entries.into_iter().unzip();
		let len = keys.len();
		if len == 0
		{
			return Self::default();
		}
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
					(first, Arc::new(Node::Branch { keys: firsts.into_iter().skip(1).collect(), kids, buf: Vec::new() }))
				})
				.collect();
		}
		BeMap { root: level.pop().map(|(_, n)| n), len }
	}

	pub fn iter(&self) -> Iter<'_, K, V>
	{
		self.seek(|_| false)
	}

	/// The entries from the first key for which the monotone `below` is false, as in `PMap::seek`
	pub fn seek(&self, below: impl Fn(&K) -> bool) -> Iter<'_, K, V>
	{
		let mut it = Iter { stack: Vec::new(), keys: &[], vals: &[], merged: None, pos: 0 };
		let Some(mut n) = self.root.as_deref() else { return it };
		let (mut lo, mut hi): (Option<&K>, Option<&K>) = (None, None);
		loop
		{
			match n
			{
				Node::Branch { keys, kids, buf } =>
				{
					let j = keys.partition_point(|s| below(s));
					it.stack.push(Frame { keys, kids, buf, i: j, hi });
					(lo, hi) = (if j > 0 { Some(&keys[j - 1]) } else { lo }, if j < keys.len() { Some(&keys[j]) } else { hi });
					n = &kids[j];
				}
				Node::Leaf { keys, vals } =>
				{
					it.load(keys, vals, lo, hi);
					it.pos = match &it.merged
					{
						Some(m) => m.partition_point(|(k, _)| below(k)),
						None => keys.partition_point(|k| below(k)),
					};
					return it;
				}
			}
		}
	}

	/// Check the structure: the B+-tree's invariants, and buffers sorted, within their node's range and at most 2 CAP long;
	/// and that the length is the number of entries a merged iteration sees
	#[cfg(test)]
	pub(crate) fn check(&self)
	{
		fn walk<K: Ord, V>(n: &Node<K, V>, root: bool, lo: Option<&K>, hi: Option<&K>) -> usize
		{
			assert!(root || n.size() >= MIN, "a node below half full");
			assert!(n.size() <= B, "an overfull node");
			let within = |k: &K| lo.map_or(true, |l| l <= k) && hi.map_or(true, |h| k < h);
			match n
			{
				Node::Leaf { keys, vals } =>
				{
					assert_eq!(keys.len(), vals.len());
					assert!(keys.windows(2).all(|w| w[0] < w[1]) && keys.iter().all(within));
					1
				}
				Node::Branch { keys, kids, buf } =>
				{
					assert_eq!(keys.len() + 1, kids.len());
					assert!(kids.len() >= 2 && keys.windows(2).all(|w| w[0] < w[1]));
					// A merge of two siblings may hold both their buffers until messages next reach it
					assert!(buf.len() <= 2 * CAP && buf.windows(2).all(|w| w[0].0 < w[1].0) && buf.iter().all(|(k, _)| within(k)), "a bad buffer");
					let mut depth = None;
					for (i, kid) in kids.iter().enumerate()
					{
						let (l, h) = (if i == 0 { lo } else { Some(&keys[i - 1]) }, if i == keys.len() { hi } else { Some(&keys[i]) });
						let d = walk(kid, false, l, h);
						assert!(depth.map_or(true, |x| x == d), "leaves at different depths");
						depth = Some(d);
					}
					1 + depth.expect("children")
				}
			}
		}
		if let Some(r) = &self.root
		{
			walk(r, true, None, None);
		}
		assert_eq!(self.iter().count(), self.len, "the length and the entries disagree");
	}
}

struct Frame<'a, K, V>
{
	keys: &'a [K],
	kids: &'a [Arc<Node<K, V>>],
	buf: &'a [(K, Msg<V>)],
	/// The child on the path
	i: usize,
	/// The branch's upper bound
	hi: Option<&'a K>,
}

pub struct Iter<'a, K, V>
{
	stack: Vec<Frame<'a, K, V>>,
	/// The current leaf's entries: its own arrays when no ancestor holds a message for its range, else merged with those messages
	keys: &'a [K],
	vals: &'a [V],
	merged: Option<Vec<(&'a K, &'a V)>>,
	pos: usize,
}

impl<'a, K: Ord, V> Iter<'a, K, V>
{
	// The leaf's entries with every ancestor's messages for [lo, hi) applied, the deepest first (so the higher, newer, ones win)
	fn load(&mut self, keys: &'a [K], vals: &'a [V], lo: Option<&'a K>, hi: Option<&'a K>)
	{
		(self.keys, self.vals, self.merged, self.pos) = (keys, vals, None, 0);
		let range = |buf: &'a [(K, Msg<V>)]| (lo.map_or(0, |l| buf.partition_point(|(k, _)| k < l)), hi.map_or(buf.len(), |h| buf.partition_point(|(k, _)| k < h)));
		if self.stack.iter().all(|f| {
			let (s, e) = range(f.buf);
			s == e
		})
		{
			return;
		}
		let mut cur: Vec<(&'a K, Option<&'a V>)> = keys.iter().zip(vals.iter().map(Some)).collect();
		for f in self.stack.iter().rev()
		{
			let (s, e) = range(f.buf);
			if s == e
			{
				continue;
			}
			let mut out = Vec::with_capacity(cur.len() + e - s);
			let mut a = cur.into_iter().peekable();
			for (k, m) in &f.buf[s..e]
			{
				while a.peek().is_some_and(|(o, _)| *o < k)
				{
					out.push(a.next().expect("peeked"));
				}
				if a.peek().is_some_and(|(o, _)| *o == k)
				{
					a.next();
				}
				out.push((k, if let Msg::Put(v) = m { Some(v) } else { None }));
			}
			out.extend(a);
			cur = out;
		}
		self.merged = Some(cur.into_iter().filter_map(|(k, v)| v.map(|v| (k, v))).collect());
	}

	fn len(&self) -> usize
	{
		self.merged.as_ref().map_or(self.keys.len(), |m| m.len())
	}

	fn at(&self, i: usize) -> (&'a K, &'a V)
	{
		match &self.merged
		{
			Some(m) => m[i],
			None => (&self.keys[i], &self.vals[i]),
		}
	}
}

impl<'a, K: Ord, V> Iterator for Iter<'a, K, V>
{
	type Item = (&'a K, &'a V);

	fn next(&mut self) -> Option<Self::Item>
	{
		while self.pos == self.len()
		{
			// Up to the first branch with a child after the path's, then down its leftmost path
			loop
			{
				let f = self.stack.last_mut()?;
				if f.i + 1 < f.kids.len()
				{
					f.i += 1;
					break;
				}
				self.stack.pop();
			}
			let f = self.stack.last().expect("just advanced");
			let lo = Some(&f.keys[f.i - 1]);
			let mut hi = if f.i < f.keys.len() { Some(&f.keys[f.i]) } else { f.hi };
			let mut n: &'a Node<K, V> = &f.kids[f.i];
			loop
			{
				match n
				{
					Node::Branch { keys, kids, buf } =>
					{
						self.stack.push(Frame { keys, kids, buf, i: 0, hi });
						hi = Some(&keys[0]);
						n = &kids[0];
					}
					Node::Leaf { keys, vals } =>
					{
						self.load(keys, vals, lo, hi);
						break;
					}
				}
			}
		}
		self.pos += 1;
		Some(self.at(self.pos - 1))
	}
}

#[cfg(test)]
mod tests
{
	use super::*;
	use crate::pmap::PMap;
	use crate::testutil::Rng;
	use std::collections::BTreeMap;

	// Random insertions of absent keys and removals of present ones against BTreeMap, every version kept and checked again at the end
	#[test]
	fn matches_btreemap_and_keeps_old_versions()
	{
		for (seed, range) in [(1, 500), (2, 60), (3, 20_000)]
		{
			let mut rng = Rng::new(seed);
			let mut model = BTreeMap::new();
			let mut map = BeMap::new();
			let mut versions = Vec::new();
			for step in 0..6000
			{
				let k = rng.next() % range;
				let removing = rng.next() % 3 == 0 || (step / 700) % 4 == 3;
				match (removing, model.contains_key(&k))
				{
					(true, true) =>
					{
						model.remove(&k);
						map.remove_present(&k);
					}
					(false, false) =>
					{
						let v = rng.next();
						model.insert(k, v);
						map.insert_new(k, v);
					}
					_ => {}
				}
				assert_eq!(map.get(&k), model.get(&k));
				if step % 50 == 0
				{
					map.check();
				}
				versions.push((map.clone(), model.clone()));
			}
			for (m, b) in versions.iter().step_by(37)
			{
				m.check();
				assert_eq!(m.len(), b.len());
				assert!(m.iter().map(|(k, v)| (*k, *v)).eq(b.iter().map(|(k, v)| (*k, *v))));
				for probe in (0..range).step_by((range / 97).max(1) as usize)
				{
					assert_eq!(m.get(&probe), b.get(&probe));
				}
			}
		}
	}

	// seek at every bound of a map whose recent updates are still buffered
	#[test]
	fn seek_every_bound_with_buffered_messages()
	{
		let mut model: BTreeMap<u64, u64> = (0..3000u64).map(|k| (k * 3, k)).collect();
		let mut map = BeMap::from_sorted(model.iter().map(|(k, v)| (*k, *v)));
		let mut rng = Rng::new(5);
		for _ in 0..400
		{
			let k = rng.next() % 9100;
			if model.contains_key(&k)
			{
				model.remove(&k);
				map.remove_present(&k);
			}
			else
			{
				model.insert(k, 7);
				map.insert_new(k, 7);
			}
		}
		map.check();
		for lo in 0..9110u64
		{
			assert!(map.seek(|k| *k < lo).map(|(k, _)| *k).eq(model.range(lo..).map(|(k, _)| *k)), "lo {lo}");
		}
	}

	// The gate: 1M scattered insertions, 1M point reads and a full scan, against the B+-tree
	// (release: cargo test --release bemap_vs_pmap -- --ignored --nocapture)
	#[test]
	#[ignore]
	fn bemap_vs_pmap()
	{
		type Key = (u64, u32, u64);
		let mut rng = Rng::new(17);
		let keys: Vec<Key> = (0..1_000_000).map(|_| (rng.next() % 5_000_000, (rng.next() % 4) as u32, rng.next())).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
		let mut order = keys.clone();
		for i in (1..order.len()).rev()
		{
			order.swap(i, (rng.next() % (i as u64 + 1)) as usize);
		}
		for round in 0..2
		{
			let t = std::time::Instant::now();
			let mut p: PMap<Key, u64> = PMap::new();
			for k in &order
			{
				p.insert_mut(*k, 1);
			}
			let p_ins = t.elapsed();
			let t = std::time::Instant::now();
			let mut b: BeMap<Key, u64> = BeMap::new();
			for k in &order
			{
				b.insert_new(*k, 1);
			}
			let b_ins = t.elapsed();
			let t = std::time::Instant::now();
			let p_hits = order.iter().filter(|k| p.get(k).is_some()).count();
			let p_get = t.elapsed();
			let t = std::time::Instant::now();
			let b_hits = order.iter().filter(|k| b.get(k).is_some()).count();
			let b_get = t.elapsed();
			let t = std::time::Instant::now();
			let p_scan = p.iter().count();
			let p_sc = t.elapsed();
			let t = std::time::Instant::now();
			let b_scan = b.iter().count();
			let b_sc = t.elapsed();
			assert_eq!((p_hits, p_scan), (b_hits, b_scan));
			println!(
				"round {round}, {} keys: insert pmap {p_ins:?} bemap {b_ins:?} ({:.2}x) | get pmap {p_get:?} bemap {b_get:?} ({:.2}x) | scan pmap {p_sc:?} bemap {b_sc:?} ({:.2}x)",
				keys.len(),
				p_ins.as_secs_f64() / b_ins.as_secs_f64(),
				b_get.as_secs_f64() / p_get.as_secs_f64(),
				b_sc.as_secs_f64() / p_sc.as_secs_f64()
			);
		}
	}
}
