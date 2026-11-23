//! The history indexes: every assertion and retraction ever applied, in one sort order each.
//!
//! A history index only grows, which static compressed structures fit: an `HIndex` is a few immutable sorted *segments* and a small
//! sorted *tail* of the latest entries. An insertion goes into the tail; a full tail becomes a segment, merged with the segments
//! that aren't at least `RATIO` times its size (so the sizes are geometric: there are logarithmically many segments, and an entry is
//! merged a logarithmic number of times). The merge works on the segments' integers, without rebuilding the entries.
//! Everything is behind `Arc`s, so a clone is two reference counts and an old value keeps reading the segments it had.
//!
//! A segment stores its entries as integers (see `succinct`):
//! * the values are replaced by their ranks in the segment's sorted dictionary, which keeps their order;
//! * each column is as wide as its largest integer, and the leading columns of an entry (as many as fit in 64 bits) are one integer.
//!   These integers are non-decreasing, and are an Elias-Fano sequence: the entries of a key prefix, say an entity's (EAVT) or an
//!   attribute's with a value (AVET), are found by two successor searches, which read a few cache lines however long the segment is,
//!   rather than by a binary search of it;
//! * the other columns and the `added` flag are packed row by row, so a scan reads consecutive bits.
//!
//! A read locates the range of a key prefix in each segment and in the tail, and merges the ranges (there is one range in the common case).
//!
//! Against the persistent treap (`pmap`) that held these indexes before: an entry takes 8 to 9 bytes rather than a node of about 90,
//! and reads by a key prefix take about half the time (the README has the timings). A long scan of one segment is no faster than
//! walking a bulk-built treap, and building the segments from a checkpoint is about 10% slower than building the treaps.

use crate::db::{AttrId, Datom, Eid, Value};
use crate::succinct::{width_of, Bits, EliasFano, EliasFanoIter};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

/// A full tail becomes a segment (a tiny one in the tests, so that the small histories of the database's tests are in many segments)
const TAIL_CAP: usize = if cfg!(test) { 8 } else { 256 };
/// A new segment is merged with the last one unless that one is at least RATIO times its size
const RATIO: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Col
{
	E,
	A,
	V,
	Tx,
}

/// A sort order of the history entries: the columns in order (the `added` flag is always last)
pub(crate) type Order = [Col; 4];
pub(crate) const EAVT: Order = [Col::E, Col::A, Col::V, Col::Tx];
pub(crate) const AEVT: Order = [Col::A, Col::E, Col::V, Col::Tx];
pub(crate) const AVET: Order = [Col::A, Col::V, Col::E, Col::Tx];
/// Of the Ref entries only
pub(crate) const VAET: Order = [Col::V, Col::A, Col::E, Col::Tx];
/// Transaction order
pub(crate) const LOG: Order = [Col::Tx, Col::E, Col::A, Col::V];

/// A history entry read from an index, which keeps the value
#[derive(Clone, Copy, Debug)]
pub(crate) struct Change<'a>
{
	pub(crate) e: Eid,
	pub(crate) a: AttrId,
	pub(crate) v: &'a Value,
	pub(crate) tx: Eid,
	pub(crate) added: bool,
}

impl<'a> Change<'a>
{
	fn of(d: &'a Datom) -> Self
	{
		Change { e: d.e, a: d.a, v: &d.v, tx: d.tx, added: d.added }
	}

	pub(crate) fn datom(&self) -> Datom
	{
		Datom { e: self.e, a: self.a, v: self.v.clone(), tx: self.tx, added: self.added }
	}
}

/// A column of a key prefix (`HIndex::scan`)
#[derive(Clone, Copy, Debug)]
pub(crate) enum Key<'a>
{
	E(Eid),
	A(AttrId),
	V(&'a Value),
	Tx(Eid),
}

fn cmp(order: &Order, x: &Change, y: &Change) -> Ordering
{
	for col in order
	{
		let o = match col
		{
			Col::E => x.e.cmp(&y.e),
			Col::A => x.a.cmp(&y.a),
			Col::V => x.v.cmp(y.v),
			Col::Tx => x.tx.cmp(&y.tx),
		};
		if o.is_ne()
		{
			return o;
		}
	}
	x.added.cmp(&y.added)
}

// The order of two history entries in an index
fn cmp_datoms(order: &Order, x: &Datom, y: &Datom) -> Ordering
{
	cmp(order, &Change::of(x), &Change::of(y))
}

// The leading columns of an entry against a key prefix
fn cmp_prefix(x: &Change, prefix: &[Key]) -> Ordering
{
	for key in prefix
	{
		let o = match key
		{
			Key::E(e) => x.e.cmp(e),
			Key::A(a) => x.a.cmp(a),
			Key::V(v) => x.v.cmp(v),
			Key::Tx(tx) => x.tx.cmp(tx),
		};
		if o.is_ne()
		{
			return o;
		}
	}
	Ordering::Equal
}

fn is_prefix_of(prefix: &[Key], order: &Order) -> bool
{
	prefix.len() <= order.len() && prefix.iter().zip(order).all(|(key, col)| matches!((key, col), (Key::E(_), Col::E) | (Key::A(_), Col::A) | (Key::V(_), Col::V) | (Key::Tx(_), Col::Tx)))
}

// x << n, for n up to 64
fn shl(x: u64, n: u32) -> u64
{
	x.checked_shl(n).unwrap_or(0)
}

/// An immutable sorted run of history entries
struct Segment
{
	len: usize,
	/// The distinct values, sorted: an entry holds the rank of its value
	dict: Vec<Value>,
	/// The bits of each column's integers
	widths: [u32; 4],
	/// The first `lead_cols` columns of each entry (as many as fit in 64 bits) as one integer, the first column in its high bits:
	/// it is non-decreasing, and the entries of a prefix of these columns are a range of its values
	lead: EliasFano,
	lead_cols: usize,
	/// The other columns and the `added` flag of each entry
	rows: Bits,
	row_bits: usize,
	/// Where each column is: its shift in the lead, or its offset in a row
	places: [u32; 4],
}

impl Segment
{
	// The segment of the entries of `segs` and of `entries` (which are in the order, and all distinct): a merge of their integers,
	// with the values' ranks mapped to the merged dictionary's
	fn merge(order: &Order, segs: &[Arc<Segment>], entries: &[Datom]) -> Segment
	{
		debug_assert!(entries.windows(2).all(|w| cmp_datoms(order, &w[0], &w[1]).is_lt()), "the entries must be strictly increasing");
		// The values of the entries (a run of a value, which the orders make common, is looked up once)
		let starts_run = |i: usize| i == 0 || entries[i - 1].v != entries[i].v;
		let mut ranks: HashMap<&Value, u64> = (0..entries.len()).filter(|i| starts_run(*i)).map(|i| (&entries[i].v, 0)).collect();
		// The dictionary: the union of the sorted dictionaries (a merge of sorted runs, to the sort)
		let mut own: Vec<&Value> = ranks.keys().copied().collect();
		own.sort_unstable();
		let mut dict: Vec<&Value> = segs.iter().flat_map(|seg| seg.dict.iter()).chain(own).collect();
		dict.sort();
		dict.dedup();
		for (v, rank) in ranks.iter_mut()
		{
			*rank = dict.binary_search(v).expect("in the union") as u64;
		}
		let v_col = order.iter().position(|col| *col == Col::V).expect("every order has the value");

		// The integers of each source's entries, in order
		type Ints = ([u64; 4], bool);
		let mut sources: Vec<Box<dyn Iterator<Item = Ints> + '_>> = Vec::new();
		for seg in segs
		{
			let mut at = 0;
			let remap: Vec<u64> = seg
				.dict
				.iter()
				.map(|v| {
					at += dict[at..].iter().position(|u| *u == v).expect("in the union");
					at as u64
				})
				.collect();
			sources.push(Box::new(seg.lead.iter_from(0).enumerate().map(move |(i, lead)| {
				let (mut ints, added) = seg.ints(i, lead);
				ints[v_col] = remap[ints[v_col] as usize];
				(ints, added)
			})));
		}
		let mut rank = 0;
		let ranks = &ranks;
		sources.push(Box::new(entries.iter().enumerate().map(move |(i, d)| {
			if starts_run(i)
			{
				rank = ranks[&d.v];
			}
			let ints = order.map(|col| match col
			{
				Col::E => d.e,
				Col::A => d.a as u64,
				Col::V => rank,
				Col::Tx => d.tx,
			});
			(ints, d.added)
		})));

		let max_of = |c: usize| match order[c]
		{
			Col::E => entries.iter().map(|d| d.e).max(),
			Col::A => entries.iter().map(|d| d.a as u64).max(),
			Col::V => Some(dict.len() as u64 - 1),
			Col::Tx => entries.iter().map(|d| d.tx).max(),
		};
		let widths = [0, 1, 2, 3].map(|c| segs.iter().map(|seg| seg.widths[c]).chain(max_of(c).map(width_of)).max().unwrap_or(0));
		let mut lead_cols = 1;
		while lead_cols < 4 && widths[..=lead_cols].iter().sum::<u32>() <= 64
		{
			lead_cols += 1;
		}
		let len = segs.iter().map(|seg| seg.len).sum::<usize>() + entries.len();
		let row_bits = widths[lead_cols..].iter().sum::<u32>() as usize + 1;
		let (mut leads, mut rows) = (Vec::with_capacity(len), Bits::with_capacity(len * row_bits));
		let mut heads: Vec<(Ints, Box<dyn Iterator<Item = Ints> + '_>)> = sources.into_iter().filter_map(|mut source| source.next().map(|first| (first, source))).collect();
		let mut previous = None;
		while let Some(i) = (0..heads.len()).min_by_key(|i| heads[*i].0)
		{
			let (ints, added) = heads[i].0;
			debug_assert!(previous.replace(heads[i].0) < Some(heads[i].0), "the entries are distinct, and the integers keep their order");
			leads.push((0..lead_cols).fold(0, |lead, c| shl(lead, widths[c]) | ints[c]));
			for c in lead_cols..4
			{
				rows.push(ints[c], widths[c]);
			}
			rows.push(added as u64, 1);
			match heads[i].1.next()
			{
				Some(next) => heads[i].0 = next,
				None =>
				{
					drop(heads.swap_remove(i));
				}
			}
		}
		debug_assert!(leads.len() == len);
		drop(heads);
		let places = [0, 1, 2, 3].map(|c| if c < lead_cols { widths[c + 1..lead_cols].iter().sum() } else { widths[lead_cols..c].iter().sum() });
		Segment { len, dict: dict.into_iter().cloned().collect(), widths, lead: EliasFano::new(&leads), lead_cols, rows, row_bits, places }
	}

	// The integer of prefix column c in this segment, unless no entry has it (a value that isn't in the dictionary, or an id beyond the column's)
	fn int_of(&self, key: &Key, c: usize) -> Option<u64>
	{
		let x = match key
		{
			Key::E(e) | Key::Tx(e) => *e,
			Key::A(a) => *a as u64,
			Key::V(v) => self.dict.binary_search(v).ok()? as u64,
		};
		(width_of(x) <= self.widths[c]).then_some(x)
	}

	// The range of the entries with the key prefix, if there are any
	fn range(&self, prefix: &[Key]) -> Option<(usize, usize)>
	{
		// The prefix columns among the lead's are a range of its values, found by two successor searches
		let (mut lo, mut hi) = (0, self.len);
		let in_lead = prefix.len().min(self.lead_cols);
		if in_lead > 0
		{
			let (mut first, mut below) = (0, self.widths[..self.lead_cols].iter().sum::<u32>());
			for (c, key) in prefix[..in_lead].iter().enumerate()
			{
				first = shl(first, self.widths[c]) | self.int_of(key, c)?;
				below -= self.widths[c];
			}
			lo = self.lead.lower_bound(shl(first, below));
			// The first value after the range may be beyond 64 bits
			hi = u64::try_from((first as u128 + 1) << below).map_or(self.len, |after| self.lead.lower_bound(after));
		}
		// Among entries equal in the columns before it, a column is in order
		let mut offset = 0;
		for (c, key) in prefix.iter().enumerate().skip(self.lead_cols)
		{
			let (x, width) = (self.int_of(key, c)?, self.widths[c]);
			let col = |i: usize| self.rows.get(i * self.row_bits + offset, width);
			let bound = |mut lo: usize, mut hi: usize, below: &dyn Fn(u64) -> bool| {
				while lo < hi
				{
					let mid = lo + (hi - lo) / 2;
					if below(col(mid)) { lo = mid + 1 } else { hi = mid }
				}
				lo
			};
			lo = bound(lo, hi, &|y| y < x);
			hi = bound(lo, hi, &|y| y <= x);
			offset += width as usize;
		}
		(lo < hi).then_some((lo, hi))
	}

	// The integers of the columns of entry i, given its lead, and its `added` flag
	#[inline]
	fn ints(&self, i: usize, lead: u64) -> ([u64; 4], bool)
	{
		let row = i * self.row_bits;
		let mut ints = [0; 4];
		for c in 0..4
		{
			// (A column of the lead whose shift is 64 has no bits)
			ints[c] = if c < self.lead_cols { lead.wrapping_shr(self.places[c]) & !shl(!0, self.widths[c]) } else { self.rows.get(row + self.places[c] as usize, self.widths[c]) };
		}
		(ints, self.rows.get(row + self.row_bits - 1, 1) == 1)
	}

	// Entry i, given its lead
	fn entry(&self, order: &Order, i: usize, lead: u64) -> Change<'_>
	{
		let (ints, added) = self.ints(i, lead);
		let mut out = Change { e: 0, a: 0, v: &self.dict[0], tx: 0, added };
		for (col, x) in order.iter().zip(ints)
		{
			match col
			{
				Col::E => out.e = x,
				Col::A => out.a = x as AttrId,
				Col::V => out.v = &self.dict[x as usize],
				Col::Tx => out.tx = x,
			}
		}
		out
	}

	#[cfg(test)]
	fn heap_bytes(&self) -> usize
	{
		let strs: usize = self.dict.iter().map(|v| if let Value::Str(s) = v { s.len() + 16 } else { 0 }).sum();
		self.dict.len() * std::mem::size_of::<Value>() + strs + self.lead.heap_bytes() + self.rows.heap_bytes()
	}
}

// The entries of a range of a segment or of the tail, in order
enum Cursor<'a>
{
	Seg
	{
		seg: &'a Segment, order: Order, i: usize, end: usize, leads: EliasFanoIter<'a>
	},
	Tail(std::slice::Iter<'a, Datom>),
}

impl<'a> Cursor<'a>
{
	// Over a range of a segment
	fn of(seg: &'a Segment, order: Order, (i, end): (usize, usize)) -> Self
	{
		Cursor::Seg { seg, order, i, end, leads: seg.lead.iter_from(i) }
	}

	fn next(&mut self) -> Option<Change<'a>>
	{
		match self
		{
			Cursor::Seg { seg, order, i, end, leads } =>
			{
				if i == end
				{
					return None;
				}
				*i += 1;
				Some(seg.entry(order, *i - 1, leads.next().expect("a lead for each entry")))
			}
			Cursor::Tail(entries) => entries.next().map(Change::of),
		}
	}
}

/// The entries of a key prefix, in index order: the merge of their ranges in the segments and the tail
pub(crate) struct Scan<'a>
{
	order: Order,
	// Each cursor with its next entry, the least entry last
	heads: Vec<(Change<'a>, Cursor<'a>)>,
}

impl<'a> Iterator for Scan<'a>
{
	type Item = Change<'a>;

	fn next(&mut self) -> Option<Change<'a>>
	{
		let last = self.heads.len().checked_sub(1)?;
		let out = self.heads[last].0;
		match self.heads[last].1.next()
		{
			// The cursor moves down to its place (it often stays: most entries are in the largest segment)
			Some(next) =>
			{
				self.heads[last].0 = next;
				let mut i = last;
				while i > 0 && cmp(&self.order, &self.heads[i].0, &self.heads[i - 1].0).is_gt()
				{
					self.heads.swap(i, i - 1);
					i -= 1;
				}
			}
			None =>
			{
				self.heads.pop();
			}
		}
		Some(out)
	}
}

/// A history index: a persistent sorted set of history entries that only grows
#[derive(Clone)]
pub(crate) struct HIndex
{
	order: Order,
	/// Oldest (and largest) first
	segs: Arc<Vec<Arc<Segment>>>,
	/// The latest entries, in the order
	tail: Arc<Vec<Datom>>,
}

impl HIndex
{
	pub(crate) fn new(order: Order) -> Self
	{
		HIndex { order, segs: Arc::default(), tail: Arc::default() }
	}

	/// The index of `entries`, which are in the order: one segment, built in linear time
	pub(crate) fn from_sorted(order: Order, entries: Vec<Datom>) -> Self
	{
		let segs = if entries.is_empty() { Vec::new() } else { vec![Arc::new(Segment::merge(&order, &[], &entries))] };
		HIndex { order, segs: Arc::new(segs), tail: Arc::default() }
	}

	#[cfg(test)]
	pub(crate) fn len(&self) -> usize
	{
		self.segs.iter().map(|s| s.len).sum::<usize>() + self.tail.len()
	}

	/// Add an absent entry
	pub(crate) fn insert(&mut self, d: Datom)
	{
		let tail = Arc::make_mut(&mut self.tail);
		let at = tail.partition_point(|x| cmp_datoms(&self.order, x, &d).is_lt());
		debug_assert!(tail.get(at).map_or(true, |x| cmp_datoms(&self.order, x, &d).is_gt()), "the entry is new");
		tail.insert(at, d);
		if tail.len() < TAIL_CAP
		{
			return;
		}
		// The tail becomes a segment, merged with the last segments while they are less than RATIO times the entries gathered
		let segs = Arc::make_mut(&mut self.segs);
		let (mut merged, mut len) = (Vec::new(), tail.len());
		while let Some(last) = segs.pop_if(|last| last.len < RATIO * len)
		{
			len += last.len;
			merged.push(last);
		}
		segs.push(Arc::new(Segment::merge(&self.order, &merged, tail)));
		tail.clear();
	}

	/// Merge the segments and the tail into one segment, which a read then searches alone (a no-op on an index that is one segment already).
	/// The entries added afterwards go to a new tail, and to segments of their own, as before
	pub(crate) fn compact(&mut self)
	{
		if self.segs.len() > 1 || !self.tail.is_empty()
		{
			self.segs = Arc::new(vec![Arc::new(Segment::merge(&self.order, &self.segs, &self.tail))]);
			self.tail = Arc::default();
		}
	}

	#[cfg(test)]
	pub(crate) fn segments(&self) -> usize
	{
		self.segs.len() + usize::from(!self.tail.is_empty())
	}

	/// The entries whose leading columns are `prefix` (which must be leading columns of the index's order), in order
	pub(crate) fn scan<'a>(&'a self, prefix: &[Key]) -> Scan<'a>
	{
		debug_assert!(is_prefix_of(prefix, &self.order), "{prefix:?} isn't a prefix of {:?}", self.order);
		let mut heads = Vec::new();
		for seg in self.segs.iter()
		{
			if let Some(range) = seg.range(prefix)
			{
				let mut cursor = Cursor::of(seg, self.order, range);
				if let Some(first) = cursor.next()
				{
					heads.push((first, cursor));
				}
			}
		}
		let lo = self.tail.partition_point(|d| cmp_prefix(&Change::of(d), prefix).is_lt());
		let hi = lo + self.tail[lo..].partition_point(|d| cmp_prefix(&Change::of(d), prefix).is_eq());
		let mut cursor = Cursor::Tail(self.tail[lo..hi].iter());
		if let Some(first) = cursor.next()
		{
			heads.push((first, cursor));
		}
		heads.sort_by(|x, y| cmp(&self.order, &y.0, &x.0));
		Scan { order: self.order, heads }
	}

	/// Whether there is an entry whose leading columns are `prefix`
	pub(crate) fn contains(&self, prefix: &[Key]) -> bool
	{
		debug_assert!(is_prefix_of(prefix, &self.order), "{prefix:?} isn't a prefix of {:?}", self.order);
		let at = self.tail.partition_point(|d| cmp_prefix(&Change::of(d), prefix).is_lt());
		self.tail.get(at).is_some_and(|d| cmp_prefix(&Change::of(d), prefix).is_eq()) || self.segs.iter().any(|seg| seg.range(prefix).is_some())
	}

	/// All the entries, in order
	pub(crate) fn iter(&self) -> Scan<'_>
	{
		self.scan(&[])
	}

	/// The bytes of the segments and the tail
	#[cfg(test)]
	pub(crate) fn heap_bytes(&self) -> usize
	{
		self.segs.iter().map(|s| s.heap_bytes()).sum::<usize>() + self.tail.len() * std::mem::size_of::<Datom>()
	}
}

#[cfg(test)]
mod tests
{
	use super::*;
	use crate::testutil::Rng;

	fn random_entry(rng: &mut Rng, refs: bool) -> Datom
	{
		let v = match rng.next() % if refs { 1 } else { 6 }
		{
			0 => Value::Ref(rng.next() % 40),
			1 => Value::Int(rng.next() as i64 % 30),
			2 => Value::Str(["", "a", "ab", "b", "zebra"][(rng.next() % 5) as usize].into()),
			3 => Value::Bool(rng.next() % 2 == 0),
			4 => Value::Float([-0.0, 0.0, 1.5, f64::NAN, f64::INFINITY][(rng.next() % 5) as usize]),
			_ => Value::Instant(rng.next() as i64 % 5),
		};
		// Few entities and attributes with many entries each, and now and then a huge id
		let e = if rng.next() % 50 == 0 { u64::MAX - rng.next() % 3 } else { rng.next() % 60 };
		Datom { e, a: (rng.next() % 5) as AttrId, v, tx: 1000 + rng.next() % 400, added: rng.next() % 2 == 0 }
	}

	fn prefixes<'a>(order: &Order, d: &'a Datom) -> Vec<Vec<Key<'a>>>
	{
		let keys: Vec<Key> = order
			.iter()
			.map(|col| match col
			{
				Col::E => Key::E(d.e),
				Col::A => Key::A(d.a),
				Col::V => Key::V(&d.v),
				Col::Tx => Key::Tx(d.tx),
			})
			.collect();
		(0..=4).map(|n| keys[..n].to_vec()).collect()
	}

	fn assert_reads(index: &HIndex, model: &[Datom], probes: &[Datom], what: &str)
	{
		assert_eq!(index.len(), model.len(), "{what}");
		assert_eq!(index.iter().map(|c| c.datom()).collect::<Vec<_>>(), model, "{what}");
		for probe in probes
		{
			for prefix in prefixes(&index.order, probe)
			{
				let want: Vec<Datom> = model.iter().filter(|d| cmp_prefix(&Change::of(d), &prefix).is_eq()).cloned().collect();
				assert_eq!(index.scan(&prefix).map(|c| c.datom()).collect::<Vec<_>>(), want, "{what}: {prefix:?}");
				assert_eq!(index.contains(&prefix), !want.is_empty(), "{what}: {prefix:?}");
			}
		}
	}

	// In every order: an index built by insertions (through many merges) reads like the sorted list of its entries, for every key prefix
	// of entries it has and of entries it doesn't; its earlier clones still read as they did; and the bulk-built index reads the same
	#[test]
	fn matches_a_sorted_list_and_keeps_old_versions()
	{
		for (o, order) in [EAVT, AEVT, AVET, VAET, LOG].iter().enumerate()
		{
			let mut rng = Rng::new(20 + o as u64);
			let mut index = HIndex::new(*order);
			let mut model: Vec<Datom> = Vec::new();
			let mut versions = Vec::new();
			while model.len() < 6000
			{
				let d = random_entry(&mut rng, *order == VAET);
				if let Err(at) = model.binary_search_by(|x| cmp_datoms(order, x, &d))
				{
					model.insert(at, d.clone());
					index.insert(d);
					if model.len() % 997 == 0 || model.len() == TAIL_CAP - 1 || model.len() == TAIL_CAP
					{
						versions.push((index.clone(), model.clone()));
					}
				}
			}
			assert!(index.segs.len() > 1 && index.segs.windows(2).all(|w| w[0].len >= RATIO * w[1].len), "{order:?}: geometric segments");
			versions.push((index, model.clone()));
			let probes: Vec<Datom> = model.iter().step_by(131).cloned().chain((0..40).map(|_| random_entry(&mut rng, false))).collect();
			for (index, model) in &versions
			{
				assert_reads(index, model, &probes, &format!("{order:?} after {} insertions", model.len()));
				assert_reads(&HIndex::from_sorted(*order, model.clone()), model, &probes, &format!("{order:?} built from {} entries", model.len()));
			}
		}
		assert_reads(&HIndex::new(EAVT), &[], &[random_entry(&mut Rng::new(1), false)], "empty");
		assert_reads(&HIndex::from_sorted(LOG, Vec::new()), &[], &[], "built empty");
	}

	// In every order: compaction leaves one segment that reads like the index it came from (whose clone still reads the same),
	// from an empty index, a tail alone, and several segments with a tail; and the insertions after it read as they would have
	#[test]
	fn compaction_keeps_the_entries()
	{
		for (o, order) in [EAVT, AEVT, AVET, VAET, LOG].iter().enumerate()
		{
			let mut rng = Rng::new(40 + o as u64);
			let mut index = HIndex::new(*order);
			let mut model: Vec<Datom> = Vec::new();
			let mut probes: Vec<Datom> = (0..20).map(|_| random_entry(&mut rng, false)).collect();
			for stop in [0, 3, TAIL_CAP, 700, 1500, 1501]
			{
				while model.len() < stop
				{
					let d = random_entry(&mut rng, *order == VAET);
					if let Err(at) = model.binary_search_by(|x| cmp_datoms(order, x, &d))
					{
						model.insert(at, d.clone());
						index.insert(d);
					}
				}
				probes.extend(model.iter().step_by(97).cloned());
				let before = index.clone();
				assert!(stop != 700 || before.segments() > 2, "{order:?}: several segments and a tail");
				index.compact();
				assert_eq!(index.segments(), usize::from(stop > 0), "{order:?} at {stop}");
				assert_reads(&index, &model, &probes, &format!("{order:?} compacted at {stop}"));
				assert_reads(&before, &model, &probes, &format!("{order:?} before the compaction at {stop}"));
				index.compact();
				assert_eq!(index.segments(), usize::from(stop > 0), "{order:?} at {stop}, twice");
			}
		}
	}
}
