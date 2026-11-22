//! A persistent (immutable, structurally shared) sorted map: a treap whose nodes are shared through `Arc`.
//!
//! Updates copy only the path from the root to the changed node, so an old version of the map stays valid and cheap to keep.
//! The priority of a node is a hash of its key, which makes the shape of the tree a function of its key set alone (independent of the insertion order).

use std::cmp::Ordering;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

type Link<K, V> = Option<Arc<Node<K, V>>>;

struct Node<K, V>
{
	key: K,
	val: V,
	prio: u64,
	left: Link<K, V>,
	right: Link<K, V>,
}

pub struct PMap<K, V>
{
	root: Link<K, V>,
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

fn prio_of<K: Hash>(key: &K) -> u64
{
	// DefaultHasher::new() uses fixed keys, so the priorities are deterministic
	let mut h = DefaultHasher::new();
	key.hash(&mut h);
	h.finish()
}

fn node<K, V>(key: K, val: V, prio: u64, left: Link<K, V>, right: Link<K, V>) -> Link<K, V>
{
	Some(Arc::new(Node { key, val, prio, left, right }))
}

impl<K: Ord + Hash + Clone, V: Clone> PMap<K, V>
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
		let mut cur = &self.root;
		while let Some(n) = cur
		{
			match key.cmp(&n.key)
			{
				Ordering::Less => cur = &n.left,
				Ordering::Greater => cur = &n.right,
				Ordering::Equal => return Some(&n.val),
			}
		}
		None
	}

	pub fn contains_key(&self, key: &K) -> bool
	{
		self.get(key).is_some()
	}

	/// A new map with `key` bound to `val` (replacing any previous binding)
	pub fn insert(&self, key: K, val: V) -> Self
	{
		let existed = self.contains_key(&key);
		let prio = prio_of(&key);
		let root = if existed { Self::replace(&self.root, key, val) } else { Self::ins(&self.root, key, val, prio) };
		PMap { root, len: if existed { self.len } else { self.len + 1 } }
	}

	/// A new map without `key` (the same map, if `key` is absent)
	pub fn remove(&self, key: &K) -> Self
	{
		if !self.contains_key(key)
		{
			return self.clone();
		}
		PMap { root: Self::rem(&self.root, key), len: self.len - 1 }
	}

	fn replace(t: &Link<K, V>, key: K, val: V) -> Link<K, V>
	{
		let n = t.as_ref().expect("the key is present");
		match key.cmp(&n.key)
		{
			Ordering::Less => node(n.key.clone(), n.val.clone(), n.prio, Self::replace(&n.left, key, val), n.right.clone()),
			Ordering::Greater => node(n.key.clone(), n.val.clone(), n.prio, n.left.clone(), Self::replace(&n.right, key, val)),
			Ordering::Equal => node(key, val, n.prio, n.left.clone(), n.right.clone()),
		}
	}

	// Insert an absent key
	fn ins(t: &Link<K, V>, key: K, val: V, prio: u64) -> Link<K, V>
	{
		match t
		{
			None => node(key, val, prio, None, None),
			Some(n) if prio > n.prio =>
			{
				let (l, r) = Self::split(t, &key);
				node(key, val, prio, l, r)
			}
			Some(n) =>
			{
				if key < n.key
				{
					node(n.key.clone(), n.val.clone(), n.prio, Self::ins(&n.left, key, val, prio), n.right.clone())
				}
				else
				{
					node(n.key.clone(), n.val.clone(), n.prio, n.left.clone(), Self::ins(&n.right, key, val, prio))
				}
			}
		}
	}

	// Split into the keys below and above an absent key
	fn split(t: &Link<K, V>, key: &K) -> (Link<K, V>, Link<K, V>)
	{
		match t
		{
			None => (None, None),
			Some(n) =>
			{
				if *key < n.key
				{
					let (l, r) = Self::split(&n.left, key);
					(l, node(n.key.clone(), n.val.clone(), n.prio, r, n.right.clone()))
				}
				else
				{
					let (l, r) = Self::split(&n.right, key);
					(node(n.key.clone(), n.val.clone(), n.prio, n.left.clone(), l), r)
				}
			}
		}
	}

	// Remove a present key
	fn rem(t: &Link<K, V>, key: &K) -> Link<K, V>
	{
		let n = t.as_ref().expect("the key is present");
		match key.cmp(&n.key)
		{
			Ordering::Less => node(n.key.clone(), n.val.clone(), n.prio, Self::rem(&n.left, key), n.right.clone()),
			Ordering::Greater => node(n.key.clone(), n.val.clone(), n.prio, n.left.clone(), Self::rem(&n.right, key)),
			Ordering::Equal => Self::merge(&n.left, &n.right),
		}
	}

	// Merge two treaps, all of whose keys in l are below those in r
	fn merge(l: &Link<K, V>, r: &Link<K, V>) -> Link<K, V>
	{
		match (l, r)
		{
			(None, _) => r.clone(),
			(_, None) => l.clone(),
			(Some(a), Some(b)) =>
			{
				if a.prio > b.prio
				{
					node(a.key.clone(), a.val.clone(), a.prio, a.left.clone(), Self::merge(&a.right, r))
				}
				else
				{
					node(b.key.clone(), b.val.clone(), b.prio, Self::merge(l, &b.left), b.right.clone())
				}
			}
		}
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
		let mut stack = Vec::new();
		let mut cur = &self.root;
		while let Some(n) = cur
		{
			if below(&n.key)
			{
				cur = &n.right;
			}
			else
			{
				stack.push(&**n);
				cur = &n.left;
			}
		}
		Iter { stack }
	}
}

pub struct Iter<'a, K, V>
{
	stack: Vec<&'a Node<K, V>>,
}

impl<'a, K, V> Iterator for Iter<'a, K, V>
{
	type Item = (&'a K, &'a V);

	fn next(&mut self) -> Option<Self::Item>
	{
		let n = self.stack.pop()?;
		let mut cur = &n.right;
		while let Some(c) = cur
		{
			self.stack.push(c);
			cur = &c.left;
		}
		Some((&n.key, &n.val))
	}
}

#[cfg(test)]
mod tests
{
	use super::*;
	use crate::testutil::Rng;
	use std::collections::BTreeMap;

	#[test]
	fn matches_btreemap_and_keeps_old_versions()
	{
		let mut rng = Rng::new(1);
		let mut model = BTreeMap::new();
		let mut map = PMap::new();
		let mut versions = Vec::new();
		for _ in 0..3000
		{
			let k = rng.next() % 500;
			if rng.next() % 3 == 0
			{
				model.remove(&k);
				map = map.remove(&k);
			}
			else
			{
				let v = rng.next();
				model.insert(k, v);
				map = map.insert(k, v);
			}
			versions.push((map.clone(), model.clone()));
		}
		for (m, b) in versions.iter().step_by(97)
		{
			assert_eq!(m.len(), b.len());
			assert!(m.iter().map(|(k, v)| (*k, *v)).eq(b.iter().map(|(k, v)| (*k, *v))));
			assert!(m.seek(|k| *k < 250).map(|(k, _)| *k).eq(b.range(250..).map(|(k, _)| *k)));
			assert_eq!(m.get(&7), b.get(&7));
		}
	}

	#[test]
	fn shape_is_independent_of_insertion_order()
	{
		fn depth(t: &Link<u32, ()>) -> usize
		{
			t.as_ref().map_or(0, |n| 1 + depth(&n.left).max(depth(&n.right)))
		}
		let a = (0..1000u32).fold(PMap::new(), |m, k| m.insert(k, ()));
		let b = (0..1000u32).rev().fold(PMap::new(), |m, k| m.insert(k, ()));
		assert_eq!(depth(&a.root), depth(&b.root));
		assert!(depth(&a.root) < 40, "expected logarithmic depth, got {}", depth(&a.root));
	}
}
