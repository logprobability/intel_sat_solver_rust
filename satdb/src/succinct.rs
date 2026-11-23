//! Succinct building blocks of the history segments (see `hist`): static structures, built once and then only read.
//!
//! They are written from the published designs that sdsl-lite also implements (its `int_vector`, `bit_vector_il` and `sd_vector`),
//! not from its code:
//! * `Bits`: integers of any width packed back to back, so a record costs the bits of its fields;
//! * `RankSelect`: a bit vector with select, whose counters are interleaved with its bits (a counter and the 448 bits it covers share
//!   one 64-byte block, so the counter that locates a bit and the bit are one cache miss), with hints that bound the search of the blocks;
//! * `EliasFano`: a non-decreasing sequence of n integers below u in about n * (2 + log2(u / n)) bits, with a successor search
//!   that reads a few blocks whatever n is (the high parts in a `RankSelect`, the low ones in `Bits`), and iteration from any index.

/// The bits of a non-negative integer: the width of a field holding values up to `max`
pub(crate) fn width_of(max: u64) -> u32
{
	u64::BITS - max.leading_zeros()
}

/// Integers of any width (up to 64 bits each) packed back to back
#[derive(Default)]
pub(crate) struct Bits
{
	words: Vec<u64>,
	/// In bits
	len: usize,
}

impl Bits
{
	pub(crate) fn with_capacity(bits: usize) -> Self
	{
		Bits { words: Vec::with_capacity(bits.div_ceil(64) + 1), len: 0 }
	}

	/// Append the low `width` bits of `v` (its other bits must be zero)
	pub(crate) fn push(&mut self, v: u64, width: u32)
	{
		debug_assert!(width <= 64 && (width == 64 || v >> width == 0));
		if width == 0
		{
			return;
		}
		let o = (self.len % 64) as u32;
		if o == 0
		{
			self.words.push(v);
		}
		else
		{
			*self.words.last_mut().expect("a partial word") |= v << o;
			if o + width > 64
			{
				self.words.push(v >> (64 - o));
			}
		}
		self.len += width as usize;
	}

	/// The `width` bits at bit `pos`
	#[inline]
	pub(crate) fn get(&self, pos: usize, width: u32) -> u64
	{
		if width == 0
		{
			return 0;
		}
		debug_assert!(pos + width as usize <= self.len);
		let (w, o) = (pos / 64, (pos % 64) as u32);
		let lo = self.words[w] >> o;
		let v = if o + width > 64 { lo | self.words[w + 1] << (64 - o) } else { lo };
		if width == 64 { v } else { v & ((1 << width) - 1) }
	}

	#[cfg(test)]
	pub(crate) fn heap_bytes(&self) -> usize
	{
		self.words.len() * 8
	}
}

/// The data bits of a block, which also holds their rank counter in its first word
const BLOCK_BITS: usize = 448;
/// A select hint for every HINT-th one (and zero)
const HINT: usize = 1024;

/// A bit vector with select
pub(crate) struct RankSelect
{
	/// Blocks of 8 words: the number of ones before the block, then its 448 bits (there is always a block after the last bit)
	blocks: Vec<u64>,
	len: usize,
	ones: usize,
	/// The block of every HINT-th one, and of every HINT-th zero
	hints: [Vec<u32>; 2],
}

impl RankSelect
{
	/// The vector of the `len` low bits of `words`
	pub(crate) fn new(words: &[u64], len: usize) -> Self
	{
		debug_assert!(len <= words.len() * 64);
		let bit = |i: usize| i < len && words[i / 64] >> (i % 64) & 1 == 1;
		let nblocks = len / BLOCK_BITS + 1;
		let mut blocks = vec![0u64; nblocks * 8];
		let mut hints = [Vec::new(), Vec::new()];
		let mut seen = [0usize; 2];
		for b in 0..nblocks
		{
			blocks[b * 8] = seen[1] as u64;
			for i in b * BLOCK_BITS..((b + 1) * BLOCK_BITS).min(len)
			{
				let v = bit(i);
				if v
				{
					let r = i - b * BLOCK_BITS;
					blocks[b * 8 + 1 + r / 64] |= 1 << (r % 64);
				}
				if seen[v as usize] % HINT == 0
				{
					hints[v as usize].push(b as u32);
				}
				seen[v as usize] += 1;
			}
		}
		RankSelect { blocks, len, ones: seen[1], hints }
	}

	pub(crate) fn len(&self) -> usize
	{
		self.len
	}

	/// The position of the k-th one (from 0)
	pub(crate) fn select1(&self, k: usize) -> usize
	{
		debug_assert!(k < self.ones);
		self.select(k, true)
	}

	/// The position of the k-th zero (from 0)
	pub(crate) fn select0(&self, k: usize) -> usize
	{
		debug_assert!(k < self.len - self.ones);
		self.select(k, false)
	}

	// The bits of value `v` before block b
	fn before(&self, b: usize, v: bool) -> usize
	{
		let ones = self.blocks[b * 8] as usize;
		if v { ones } else { b * BLOCK_BITS - ones }
	}

	fn select(&self, k: usize, v: bool) -> usize
	{
		// The last block with at most k such bits before it, between the hinted blocks
		let hints = &self.hints[v as usize];
		let (mut lo, mut hi) = (hints[k / HINT] as usize, hints.get(k / HINT + 1).map_or(self.blocks.len() / 8 - 1, |b| *b as usize));
		while lo < hi
		{
			let mid = lo + (hi - lo + 1) / 2;
			if self.before(mid, v) <= k { lo = mid } else { hi = mid - 1 }
		}
		let mut k = k - self.before(lo, v);
		for (i, w) in self.blocks[lo * 8 + 1..lo * 8 + 8].iter().enumerate()
		{
			let mut w = if v { *w } else { !*w };
			let n = w.count_ones() as usize;
			if k < n
			{
				// Clear the k lowest set bits
				for _ in 0..k
				{
					w &= w - 1;
				}
				return lo * BLOCK_BITS + i * 64 + w.trailing_zeros() as usize;
			}
			k -= n;
		}
		unreachable!("the block holds the bit")
	}

	/// The positions of the ones from `pos` on, in order
	pub(crate) fn ones_from(&self, pos: usize) -> Ones<'_>
	{
		let pos = pos.min(self.len);
		let (b, r) = (pos / BLOCK_BITS, pos % BLOCK_BITS);
		let at = b * 8 + 1 + r / 64;
		Ones { blocks: &self.blocks, at, word: self.blocks[at] & (!0 << (r % 64)), base: pos - r % 64 }
	}

	#[cfg(test)]
	pub(crate) fn heap_bytes(&self) -> usize
	{
		self.blocks.len() * 8 + (self.hints[0].len() + self.hints[1].len()) * 4
	}
}

/// The positions of the ones of a `RankSelect` from some position on
pub(crate) struct Ones<'a>
{
	blocks: &'a [u64],
	/// The index of the current word, what is left of it, and the position of its first bit
	at: usize,
	word: u64,
	base: usize,
}

impl Iterator for Ones<'_>
{
	type Item = usize;

	#[inline]
	fn next(&mut self) -> Option<usize>
	{
		while self.word == 0
		{
			// The next word of bits (a block's first word is its counter)
			self.at += if self.at % 8 == 7 { 2 } else { 1 };
			self.word = *self.blocks.get(self.at)?;
			self.base += 64;
		}
		let pos = self.base + self.word.trailing_zeros() as usize;
		self.word &= self.word - 1;
		Some(pos)
	}
}

/// A non-decreasing sequence of integers (Elias-Fano): each value's low bits verbatim, and its high part in unary in a bit vector,
/// where the value of index i with high part h is the one at position h + i
pub(crate) struct EliasFano
{
	len: usize,
	max: u64,
	low_width: u32,
	low: Bits,
	high: RankSelect,
}

impl EliasFano
{
	pub(crate) fn new(vals: &[u64]) -> Self
	{
		debug_assert!(vals.windows(2).all(|w| w[0] <= w[1]), "the values must be non-decreasing");
		let len = vals.len();
		let max = vals.last().copied().unwrap_or(0);
		// About log2(u / n) low bits, which leaves at most 2n bits of high parts
		let low_width = if len == 0 { 0 } else { width_of(max / len as u64).saturating_sub(1) };
		let mut low = Bits::with_capacity(len * low_width as usize);
		let high_len = len + (max >> low_width) as usize + 1;
		let mut high = vec![0u64; high_len.div_ceil(64)];
		for (i, v) in vals.iter().enumerate()
		{
			low.push(if low_width == 0 { 0 } else { v & ((1 << low_width) - 1) }, low_width);
			let p = (v >> low_width) as usize + i;
			high[p / 64] |= 1 << (p % 64);
		}
		EliasFano { len, max, low_width, low, high: RankSelect::new(&high, high_len) }
	}

	fn low(&self, i: usize) -> u64
	{
		self.low.get(i * self.low_width as usize, self.low_width)
	}

	/// The index of the first value at or above `x` (the length, if there is none)
	pub(crate) fn lower_bound(&self, x: u64) -> usize
	{
		if self.len == 0 || x > self.max
		{
			return self.len;
		}
		// The values with x's high part h are those between the zeros h - 1 and h; their low bits are in order
		let h = (x >> self.low_width) as usize;
		let (mut lo, mut hi) = (if h == 0 { 0 } else { self.high.select0(h - 1) + 1 - h }, self.high.select0(h) - h);
		let x_low = if self.low_width == 0 { 0 } else { x & ((1 << self.low_width) - 1) };
		while lo < hi
		{
			let mid = lo + (hi - lo) / 2;
			if self.low(mid) < x_low { lo = mid + 1 } else { hi = mid }
		}
		lo
	}

	/// The values from index `from` on
	pub(crate) fn iter_from(&self, from: usize) -> EliasFanoIter<'_>
	{
		EliasFanoIter { seq: self, i: from, high: self.high.ones_from(if from < self.len { self.high.select1(from) } else { self.high.len() }) }
	}

	#[cfg(test)]
	pub(crate) fn heap_bytes(&self) -> usize
	{
		self.low.heap_bytes() + self.high.heap_bytes()
	}
}

pub(crate) struct EliasFanoIter<'a>
{
	seq: &'a EliasFano,
	/// The next index, and the bits of the high parts from its bit on
	i: usize,
	high: Ones<'a>,
}

impl Iterator for EliasFanoIter<'_>
{
	type Item = u64;

	#[inline]
	fn next(&mut self) -> Option<u64>
	{
		let v = ((self.high.next()? - self.i) as u64) << self.seq.low_width | self.seq.low(self.i);
		self.i += 1;
		Some(v)
	}
}

#[cfg(test)]
mod tests
{
	use super::*;
	use crate::testutil::Rng;

	#[test]
	fn bits_round_trip_every_width()
	{
		let mut rng = Rng::new(5);
		let mut bits = Bits::default();
		let mut model = Vec::new();
		for i in 0..5000
		{
			let width = (rng.next() % 65) as u32;
			// The extremes of the width now and then
			let v = match (width, i % 3)
			{
				(0, _) => 0,
				(64, 0) => u64::MAX,
				(64, _) => rng.next(),
				(_, 0) => (1 << width) - 1,
				_ => rng.next() & ((1 << width) - 1),
			};
			model.push((bits.len, width, v));
			bits.push(v, width);
		}
		for (pos, width, v) in model
		{
			assert_eq!(bits.get(pos, width), v, "{width} bits at {pos}");
		}
		assert_eq!(width_of(0), 0);
		assert_eq!(width_of(1), 1);
		assert_eq!(width_of(255), 8);
		assert_eq!(width_of(256), 9);
		assert_eq!(width_of(u64::MAX), 64);
	}

	// Select and the iteration of the ones against a plain scan, for lengths around the block and hint boundaries and for several densities
	#[test]
	fn rank_select_match_a_scan()
	{
		let mut rng = Rng::new(6);
		for len in [0usize, 1, 63, 64, 447, 448, 449, 896, 5000, 448 * 64, 300_000]
		{
			for density in [0, 1, 2, 50, 98, 100]
			{
				let bits: Vec<bool> = (0..len).map(|_| rng.next() % 100 < density).collect();
				let mut words = vec![0u64; len.div_ceil(64)];
				for (i, _) in bits.iter().enumerate().filter(|(_, b)| **b)
				{
					words[i / 64] |= 1 << (i % 64);
				}
				let rs = RankSelect::new(&words, len);
				assert_eq!((rs.len(), rs.ones), (len, bits.iter().filter(|b| **b).count()));
				let (mut seen, mut next) = ([0usize; 2], Vec::new());
				let step = if len > 10_000 { 7 } else { 1 };
				for (i, b) in bits.iter().enumerate()
				{
					if seen[*b as usize] % step == 0
					{
						assert_eq!(if *b { rs.select1(seen[1]) } else { rs.select0(seen[0]) }, i, "select{}({}) of {len} bits at {density}%", *b as u8, seen[*b as usize]);
					}
					seen[*b as usize] += 1;
					if *b
					{
						next.push(i);
					}
				}
				assert!(rs.ones_from(0).eq(next.iter().copied()), "the ones of {len} bits at {density}%");
				for i in (0..=len + 1).step_by(step)
				{
					let want = &next[next.partition_point(|p| *p < i)..];
					assert!(rs.ones_from(i).take(600).eq(want.iter().take(600).copied()), "ones_from({i}) of {len} bits at {density}%");
				}
			}
		}
	}

	// The successor search and iteration against the plain sequence: dense, sparse, with long runs of equal values
	// and with values at the ends of the u64 range
	#[test]
	fn elias_fano_matches_the_sequence()
	{
		let mut rng = Rng::new(7);
		let mut cases: Vec<Vec<u64>> = vec![vec![], vec![0], vec![u64::MAX], vec![0, 0, 0], vec![5; 2000], vec![0, u64::MAX], vec![u64::MAX - 1, u64::MAX, u64::MAX]];
		for (n, universe) in [(1, 10), (10, 10), (1000, 10), (1000, 1000), (1000, 1 << 20), (3000, u64::MAX), (50_000, 600), (50_000, 1 << 40)]
		{
			let mut v: Vec<u64> = (0..n).map(|_| rng.next() % universe).collect();
			v.sort_unstable();
			cases.push(v);
		}
		// A few values with many entries each, among many with one
		let mut skewed: Vec<u64> = (0..20_000).map(|i| if i % 3 == 0 { 1_000_000 + rng.next() % 5 } else { rng.next() % (1 << 30) }).collect();
		skewed.sort_unstable();
		cases.push(skewed);
		for vals in cases
		{
			let ef = EliasFano::new(&vals);
			let step = if vals.len() > 5000 { 13 } else { 1 };
			assert!(ef.iter_from(0).eq(vals.iter().copied()));
			for from in [1, 2, vals.len() / 2, vals.len().saturating_sub(1), vals.len(), vals.len() + 3]
			{
				assert!(ef.iter_from(from).eq(vals.iter().skip(from).copied()), "iter_from({from}) of {} values", vals.len());
			}
			// The values themselves, their neighbours and random ones
			let mut probes: Vec<u64> = vec![0, 1, u64::MAX, u64::MAX - 1];
			for v in vals.iter().step_by(step)
			{
				probes.extend([*v, v.saturating_sub(1), v.saturating_add(1)]);
			}
			probes.extend((0..200).map(|_| rng.next() % vals.last().copied().unwrap_or(1).max(1)));
			for x in probes
			{
				assert_eq!(ef.lower_bound(x), vals.partition_point(|v| *v < x), "lower_bound({x}) of {} values", vals.len());
			}
		}
	}

	// The point of the encoding: a sorted sequence costs a few bits per value more than log2 of the average gap
	#[test]
	fn elias_fano_is_small()
	{
		let mut rng = Rng::new(8);
		let mut vals: Vec<u64> = (0..100_000).map(|_| rng.next() % (1 << 24)).collect();
		vals.sort_unstable();
		// log2(2^24 / 10^5) is 7.4: 7 low bits, 2 high bits and the rank counters
		let bits = EliasFano::new(&vals).heap_bytes() as f64 * 8.0 / vals.len() as f64;
		assert!(bits < 10.5, "{bits} bits per value");
	}
}
