// Unit tests: ToporDynArray, ToporVector, ToporBitArray, ToporBitArrayBuffer, ToporTableBitInfo, ToporWinAverage

use crate::topor_bit_array::CBitArray;
use crate::topor_bit_array_buffer::*;
use crate::topor_dyn_array::*;
use crate::topor_table_bit_info::*;
use crate::topor_vector::CVector;
use crate::topor_win_average::TWinAverage;

// Runs f, where the allocation number failAt (counting from 0, relative to the invocation) and all the subsequent ones fail
pub fn with_alloc_failure<R>(failAt: u64, f: impl FnOnce() -> R) -> R
{
	alloc_fault::fail_after(failAt);
	let r = f();
	alloc_fault::never_fail();
	r
}

#[test]
fn span_basics()
{
	let mut v = vec![1u32, 2, 3, 4, 5];
	let s = Span::from_slice(&mut v);
	assert_eq!(s.size(), 5);
	assert!(!s.is_empty());
	assert_eq!(s.get(1), 2);
	assert_eq!(s[2], 3);
	s.set(0, 10);
	s.swap(0, 4);
	assert_eq!(s.back(), 10);
	s.set_back(7);
	assert_eq!(s.to_vec(), vec![5, 2, 3, 4, 7]);
	assert_eq!(s.subspan(1, 2).to_vec(), vec![2, 3]);
	assert_eq!(s.subspan_from(3).to_vec(), vec![4, 7]);
	assert_eq!(s.find(4), 3);
	assert_eq!(s.find(100), 5);
	assert!(s.contains(7) && !s.contains(8));
	assert_eq!(s.iter().sum::<u32>(), 21);
	assert_eq!(s.into_iter().count(), 5);
	s.as_mut_slice()[0] = 9;
	assert_eq!(s.as_slice()[0], 9);
	assert_eq!(unsafe { *s.ptr() }, 9);
	let c = Span::from_const_slice(&v);
	assert_eq!(c.size(), 5);
	let e: Span<u32> = Span::default();
	assert!(e.is_empty());
	#[allow(clippy::clone_on_copy)] // Covers the Clone impl
	let s2 = s.clone();
	assert_eq!(s2.size(), 5);
}

#[test]
fn dyn_array_basics()
{
	let mut a: CDynArray<u32> = CDynArray::new(4);
	assert_eq!(a.cap(), 4);
	assert!(!a.empty() && !a.uninitialized_or_erroneous());
	a.memset(0);
	a[1] = 5;
	assert_eq!(a[1], 5);
	a.memset_from(0xff, 2);
	assert_eq!(a[3], u32::MAX);
	a.memset_range(0, 2, 3);
	assert_eq!(a[2], 0);
	assert_eq!(a.memMb(), 0);

	// Copy constructor & assignment
	let b = a.clone();
	assert_eq!(b[1], 5);
	let mut c: CDynArray<u32> = CDynArray::default();
	assert!(c.empty() && c.uninitialized_or_erroneous());
	c.SetMultiplier(2.);
	c.assign(&a);
	assert_eq!(c[3], u32::MAX);
	assert_eq!(c.GetMultiplier(), MultiplierDef);
	let d = CDynArray::<u32>::default().clone();
	assert!(d.uninitialized_or_erroneous());
	let mut e: CDynArray<u32> = CDynArray::new(3);
	e.assign(&CDynArray::default());
	assert!(e.uninitialized_or_erroneous());

	// Initialized allocations
	let z: CDynArray<u64> = CDynArray::new_init(3, 0);
	assert_eq!((z[0], z[2]), (0, 0));
	let o: CDynArray<u8> = CDynArray::new_init(3, 7);
	assert_eq!((o[0], o[2]), (7, 7));
	assert!(CDynArray::<u8>::new_init(0, 7).uninitialized_or_erroneous());
	assert!(CDynArray::<u64>::new(usize::MAX).uninitialized_or_erroneous());
	assert!(CDynArray::<u64>::new_init(usize::MAX, 0).uninitialized_or_erroneous());
	// Beyond the maximal object size, but within MaxCapacity
	assert!(CDynArray::<u64>::new(usize::MAX / 8).uninitialized_or_erroneous());

	// Reservations
	a.reserve_exactly(8);
	assert_eq!((a.cap(), a[1]), (8, 5));
	a.reserve_exactly_init(12, 1);
	assert_eq!((a.cap(), a[1], a[11]), (12, 5, 0x01010101));
	a.reserve_exactly_init(6, 1);
	assert_eq!(a.cap(), 6);
	a.reserve_atleast(10);
	assert_eq!(a.cap(), (10. * MultiplierDef + 2.) as usize);
	a.reserve_atleast_init(20, 0);
	assert_eq!(a[a.cap() - 1], 0);
	a.reserve_atleast_with_max(30, 31);
	assert_eq!(a.cap(), 31);
	a.reserve_atleast_with_max(30, 1000);
	assert_eq!(a.cap(), 50);
	a.reserve_atleast_with_max_init(40, 41, 0);
	assert_eq!(a.cap(), 41);
	a.reserve_atleast_with_max_init(40, 1000, 0);
	assert_eq!(a.cap(), 67);
	a.reserve_beyond_if_requried(10, true);
	assert_eq!(a.cap(), 67);
	a.reserve_beyond_if_requried(67, false);
	assert_eq!(a.cap(), 68);
	a.reserve_beyond_if_requried(68, true);
	assert_eq!(a.cap(), (69. * MultiplierDef + 2.) as usize);
	a.reserve_exactly(0);
	assert!(a.uninitialized_or_erroneous() && a.cap() == 0);
	a.reserve_exactly_init(0, 0);
	assert!(a.uninitialized_or_erroneous());
	a.reserve_exactly_init(2, 3);
	assert_eq!(a[1], 0x03030303);
	a.reserve_exactly(usize::MAX);
	assert!(a.uninitialized_or_erroneous());
	a.reserve_exactly_init(usize::MAX, 0);
	assert!(a.uninitialized_or_erroneous());
	a.SetMultiplier(1e300);
	a.reserve_atleast(usize::MAX / 2);
	assert!(a.uninitialized_or_erroneous());

	// Spans & pointers
	let mut s: CDynArray<u32> = CDynArray::new_init(6, 0);
	for i in 0..6
	{
		s[i] = i as u32;
	}
	assert_eq!(s.get_span_cap(2).to_vec(), vec![2, 3, 4, 5]);
	assert_eq!(s.get_span_cap_sz(1, 2).to_vec(), vec![1, 2]);
	assert_eq!(unsafe { *s.get_ptr(3) }, 3);
	assert_eq!(unsafe { *s.get_ptr_no_assert(4) }, 4);
	s.memcpy(0, 4, 2);
	assert_eq!(s.get_span_cap(0).to_vec(), vec![4, 5, 2, 3, 4, 5]);
	s.memmove(1, 0, 4);
	assert_eq!(s.get_span_cap(0).to_vec(), vec![4, 4, 5, 2, 3, 5]);
	s.memmove(0, 4, 2);
	assert_eq!(s.get_span_cap(0).to_vec(), vec![3, 5, 5, 2, 3, 5]);
	s.remove_if_equal_and_cut_capacity(5);
	assert_eq!(s.get_span_cap(0).to_vec(), vec![3, 2, 3]);
}

#[test]
fn dyn_array_alloc_failures()
{
	assert!(with_alloc_failure(0, || CDynArray::<u32>::new(4)).uninitialized_or_erroneous());
	assert!(with_alloc_failure(0, || CDynArray::<u32>::new_init(4, 0)).uninitialized_or_erroneous());
	assert!(with_alloc_failure(0, || CDynArray::<u32>::new_init(4, 1)).uninitialized_or_erroneous());
	let mut a: CDynArray<u32> = CDynArray::new(4);
	with_alloc_failure(0, || a.reserve_exactly(8));
	assert!(a.uninitialized_or_erroneous() && a.cap() == 0);
	let mut a: CDynArray<u32> = CDynArray::new(4);
	with_alloc_failure(0, || a.reserve_exactly_init(8, 0));
	assert!(a.uninitialized_or_erroneous() && a.cap() == 0);
	with_alloc_failure(0, || a.reserve_exactly_init(8, 0));
	assert!(a.uninitialized_or_erroneous() && a.cap() == 0);
	let a: CDynArray<u32> = CDynArray::new(4);
	assert!(with_alloc_failure(0, || a.clone()).uninitialized_or_erroneous());
	let mut b: CDynArray<u32> = CDynArray::new(1);
	with_alloc_failure(0, || b.assign(&a));
	assert!(b.uninitialized_or_erroneous() && b.cap() == 0);
	// The failure counter
	let before = alloc_fault::allocs_so_far();
	let v = with_alloc_failure(1, || (CDynArray::<u32>::new(1).uninitialized_or_erroneous(), CDynArray::<u32>::new(1).uninitialized_or_erroneous()));
	assert_eq!(v, (false, true));
	assert_eq!(alloc_fault::allocs_so_far(), before + 2);
}

#[test]
fn dyn_array_remove_garbage()
{
	// Chunks are [size, payload...]; a chunk is deleted iff its first payload entry is 0
	struct Ctx
	{
		a: CDynArray<u32>,
		moves: Vec<(usize, usize)>,
	}
	let layout: [&[u32]; 7] = [&[2, 1, 1], &[1, 0], &[3, 2, 2, 2], &[2, 0, 9], &[1, 0], &[1, 3], &[2, 4, 4]];
	let build = ||
	{
		let mut ctx = Ctx { a: CDynArray::new_init(64, 0), moves: vec![] };
		let mut n = 1;
		for c in layout
		{
			for x in c
			{
				ctx.a[n] = *x;
				n += 1;
			}
		}
		(ctx, n)
	};
	let (mut ctx, mut end) = build();
	RemoveGarbage(&mut ctx, |c| &mut c.a, 1, &mut end, &mut |c: &mut Ctx, i| c.a[i + 1] == 0, &mut |c: &mut Ctx, i| i + 1 + c.a[i] as usize, Some(&mut |c: &mut Ctx, from, to| c.moves.push((from, to))));
	assert_eq!(ctx.a.get_span_cap_sz(1, end - 1).to_vec(), vec![2, 1, 1, 3, 2, 2, 2, 1, 3, 2, 4, 4]);
	assert_eq!(ctx.moves, vec![(1, 1), (6, 4), (15, 8), (17, 10)]);
	// No notification; nothing deleted and everything deleted
	let (mut ctx, mut end) = build();
	RemoveGarbage(&mut ctx, |c| &mut c.a, 1, &mut end, &mut |_: &mut Ctx, _| false, &mut |c: &mut Ctx, i| i + 1 + c.a[i] as usize, None);
	assert_eq!(end, 20);
	RemoveGarbage(&mut ctx, |c| &mut c.a, 1, &mut end, &mut |_: &mut Ctx, _| true, &mut |c: &mut Ctx, i| i + 1 + c.a[i] as usize, None);
	assert_eq!(end, 1);
}

#[test]
fn vector_basics()
{
	let mut v: CVector<u32> = CVector::default();
	assert!(v.empty() && v.size() == 0);
	for i in 0..10
	{
		v.push_back(i);
	}
	v.emplace_back(10);
	assert_eq!(v.size(), 11);
	assert_eq!(v.back(), 10);
	*v.back_mut() = 11;
	assert_eq!(v.pop_back(), 11);
	assert_eq!(v.get_span().to_vec(), (0..10).collect::<Vec<u32>>());
	assert_eq!(v.get_span_from(8).to_vec(), vec![8, 9]);
	assert_eq!(v.get_span_sz(2, 3).to_vec(), vec![2, 3, 4]);
	let mut ext = vec![100u32, 101, 102];
	v.append(Span::from_slice(&mut ext));
	assert_eq!(v.size(), 13);
	// An append requiring a reallocation
	let mut big: Vec<u32> = (0..100).collect();
	v.append(Span::from_slice(&mut big));
	assert_eq!((v.size(), v.back()), (113, 99));
	v.resize(13);
	v.erase_if_may_reorder(|x| x % 2 == 0, 0);
	let mut rest = v.get_span().to_vec();
	rest.sort();
	assert_eq!(rest, vec![1, 3, 5, 7, 9, 101]);
	v.erase_if_may_reorder(|x| x > 5, 3);
	assert_eq!(v.size(), 3 + v.get_span_from(3).iter().filter(|x| *x <= 5).count());
	let w = v.clone();
	assert_eq!(w.size(), v.size());
	let mut x: CVector<u32> = CVector::new(2, 0);
	x.assign(&v);
	assert_eq!(x.get_span().to_vec(), v.get_span().to_vec());
	let mut applied = vec![];
	x.clear_apply(|e| applied.push(e));
	assert_eq!(applied.len(), v.size());
	assert!(x.empty());
	x.clear_apply(|_| panic!("the vector is empty"));
	v.resize(1000);
	assert_eq!(v.size(), 1000);
	v.clear();
	assert!(v.empty());
	let i: CVector<u8> = CVector::new_init(4, 9, 2);
	assert_eq!((i.size(), i.back()), (2, 9));
}

#[test]
fn vector_alloc_failures()
{
	let mut v: CVector<u32> = CVector::new(1, 0);
	v.push_back(1);
	with_alloc_failure(0, || v.push_back(2));
	assert!(v.uninitialized_or_erroneous());
	let mut v: CVector<u32> = CVector::new(1, 0);
	let mut ext = vec![100u32, 101, 102];
	with_alloc_failure(0, || v.append(Span::from_slice(&mut ext)));
	assert!(v.uninitialized_or_erroneous());
	let mut v: CVector<u32> = CVector::new(1, 0);
	with_alloc_failure(0, || v.resize(10));
	assert!(v.uninitialized_or_erroneous());
}

#[test]
fn table_bit_info()
{
	assert_eq!(LsbMask(0), 0);
	assert_eq!(LsbMask(64), u64::MAX);
	let i = &cMasksByBit[10 - 1][60];
	assert_eq!(i.w0ReadMask, !LsbMask(60));
	assert_eq!(i.w1ReadMask, LsbMask(6));
	assert_eq!((i.w0ReadShr, i.w1ReadShl), (60, 4));
	assert_eq!(i.W0CleanMask(), LsbMask(60));
	assert_eq!(i.W1CleanMask(), !LsbMask(6));
	assert_eq!(i.ElemW0Mask(), 0xf);
	assert_eq!(i.ElemW0Shl(), 60);
	assert_eq!(i.ElemW1Mask(), 0x3f0);
	assert_eq!(i.ElemW1Shr(), 4);
}

#[test]
fn bit_array_buffer_exhaustive_widths()
{
	// Every width at every bit offset: write three consecutive elements and read them back; the neighbours must stay intact
	for width in 1..=64u8
	{
		for start in 0..64u64
		{
			let mut buf = [0xaaaaaaaaaaaaaaaau64; 5];
			let mask = LsbMask(width);
			let vals = [0x0123456789abcdefu64 & mask, 0xfedcba9876543210u64 & mask, mask];
			let mut b = start;
			for v in vals
			{
				unsafe { CSetElemAndGoToNext(buf.as_mut_ptr(), &mut b, width, v) };
			}
			assert_eq!(b, start + 3 * width as u64);
			let mut b = start;
			for v in vals
			{
				assert_eq!(unsafe { CGetElemAndGoToNext(buf.as_ptr(), &mut b, width) }, v);
			}
			for bit in (0..start).chain(b..320)
			{
				assert_eq!((buf[(bit >> 6) as usize] >> (bit & 63)) & 1, bit & 1, "width {} start {} bit {}", width, start, bit);
			}
			unsafe { CSetElem(buf.as_mut_ptr(), start, width, 0) };
			assert_eq!(unsafe { CGetElem(buf.as_ptr(), start, width) }, 0);
		}
	}
}

#[test]
fn bit_array_basics()
{
	let mut ba = CBitArray::new();
	assert_eq!(ba.bit_get_next_bit(), 0);
	ba.bit_reserve_new_chunk(200, 0);
	let cap = ba.cap();
	assert!(cap >= 4);
	ba.bit_reserve_new_chunk(10, 0);
	assert_eq!(ba.cap(), cap);
	for i in 0..20u64
	{
		ba.bit_push(i, 7);
	}
	assert_eq!(ba.bit_get_next_bit(), 140);
	assert_eq!(ba.bit_get(7 * 5, 7), 5);
	ba.bit_set(99, 7, 7 * 5);
	let mut b = 7 * 5;
	assert_eq!(ba.bit_get_and_advance(&mut b, 7), 99);
	assert_eq!(b, 7 * 6);
	ba.bit_set_and_advance(98, 7, &mut b);
	assert_eq!((b, ba.bit_get(7 * 6, 7)), (7 * 7, 98));
	ba.copy(0, 7, 7);
	assert_eq!(ba.bit_get(7, 7), 0);
	ba.bit_remove_last_bits(7);
	assert_eq!(ba.bit_get_next_bit(), 133);
	ba.bit_set_next_bit(140);
	ba.compress();
	assert_eq!(ba.cap(), 4);
	ba.bit_resize_and_compress(0);
	assert!(ba.uninitialized_or_erroneous());
}

#[test]
fn bit_array_copy_block()
{
	// Compare against a bit-by-bit reference for all the alignment combinations
	let total: u64 = 64 * 8;
	let pattern = |i: u64| ((i * 2654435761u64) >> 7) & 1;
	for (from, to, width) in [(128u64, 0u64, 40u64), (128, 0, 64), (128, 0, 200), (128, 64, 128), (192, 64, 131), (130, 3, 200), (130, 64, 200), (67, 5, 64 * 3), (64, 3, 65), (200, 190, 150), (192, 128, 300)]
	{
		let mut ba = CBitArray::new();
		ba.bit_reserve_new_chunk(total, 0);
		for i in 0..total
		{
			ba.bit_push(pattern(i), 1);
		}
		let mut expected: Vec<u64> = (0..total).map(pattern).collect();
		for i in 0..width
		{
			expected[(to + i) as usize] = expected[(from + i) as usize];
		}
		ba.copy_block(from, to, width);
		for i in 0..total
		{
			assert_eq!(ba.bit_get(i, 1), expected[i as usize], "from {} to {} width {} bit {}", from, to, width, i);
		}
	}
}

#[test]
fn win_average()
{
	let mut w = TWinAverage::new();
	assert!(w.IsError());
	w.Enqueue(5);
	w.Init(3);
	assert!(!w.IsError());
	assert_eq!(w.MaxCap(), 3);
	w.Enqueue(1);
	w.Enqueue(2);
	assert!(!w.IsFullWindow());
	assert_eq!(w.GetAverage(), 1.5);
	w.Enqueue(3);
	assert!(w.IsFullWindow());
	assert_eq!(w.GetAverage(), 2.);
	w.Enqueue(7);
	assert_eq!(w.GetAverage(), 4.);
	w.Clear(false);
	assert!(!w.IsFullWindow());
	w.Enqueue(4);
	assert_eq!(w.GetAverage(), 4.);
	w.Clear(true);
	assert!(w.IsError());
}

#[test]
fn table_bit_info_run_time_builders()
{
	// The tables are built at compile time; building them at run time must give the same tables
	let byBit = BuildTableByBitAtRunTime();
	let byEntry = BuildTableByEntryAtRunTime();
	for w in 0..MAX_BITS_NUM as usize
	{
		assert_eq!(byEntry[w].0, tableByEntry[w].0);
		for b in 0..64
		{
			let (x, y) = (&byBit[w][b], &tableByBit[w][b]);
			assert_eq!((x.w0ReadMask, x.w1ReadMask, x.w0ReadShr), (y.w0ReadMask, y.w1ReadMask, y.w0ReadShr));
			let (x, y) = (&byEntry[w].1[b], &tableByEntry[w].1[b]);
			assert_eq!((x.w0ReadMask, x.w1ReadMask, x.w0ReadShr), (y.w0ReadMask, y.w1ReadMask, y.w0ReadShr));
		}
	}
	let i = &tableByBit[BitsInd(26)][52];
	assert_eq!((i.W0WriteMask(), i.W0WriteShl(), i.W1WriteMask(), i.W0CleanMask(), i.W1CleanMask()), (!LsbMask(14), 38, LsbMask(14), LsbMask(52), !LsbMask(14)));
}

#[test]
fn non_contagious_bit_array_buffer_exhaustive_widths()
{
	// Every width at every bit offset: write three consecutive elements and read them back; the neighbours must stay intact
	for width in 1..=64u8
	{
		for start in 0..64u64
		{
			let mut buf = [0xaaaaaaaaaaaaaaaau64; 5];
			let mask = LsbMask(width);
			let vals = [0x0123456789abcdefu64 & mask, 0xfedcba9876543210u64 & mask, mask];
			let mut b = start;
			for v in vals
			{
				unsafe { BASetElemAndGoToNext(buf.as_mut_ptr(), &mut b, width, v) };
			}
			assert_eq!(b, start + 3 * width as u64);
			let mut b = start;
			for v in vals
			{
				assert_eq!(unsafe { BAGetElemAndGoToNext(buf.as_ptr(), &mut b, width) }, v);
			}
			// The words, which the elements don't touch, are intact
			for w in 0..5u64
			{
				if w < start / 64 || w > (b - 1) / 64 + 1
				{
					assert_eq!(buf[w as usize], 0xaaaaaaaaaaaaaaaa, "width {} start {} word {}", width, start, w);
				}
			}
			// Within a word, the non-contagious and the contagious layouts agree
			if start + width as u64 <= 64
			{
				let mut cbuf = [0u64; 2];
				unsafe { CSetElem(cbuf.as_mut_ptr(), start, width, vals[0]) };
				let mut nbuf = [0u64; 2];
				unsafe { BASetElem(nbuf.as_mut_ptr(), start, width, vals[0]) };
				assert_eq!(cbuf, nbuf);
			}
			unsafe { BASetElem(buf.as_mut_ptr(), start, width, 0) };
			assert_eq!(unsafe { BAGetElem(buf.as_ptr(), start, width) }, 0);
			if width < 64
			{
				// The sum is returned as is, while only its width bits are stored
				assert_eq!(unsafe { BAIncrAndGet(buf.as_mut_ptr(), start, width, 5) }, 5);
				assert_eq!(unsafe { BAGetElem(buf.as_ptr(), start, width) }, 5 & mask);
			}
		}
	}
}

#[test]
fn non_contagious_bit_array_buffer_const_expr()
{
	fn Check<const S: u8, const W: u8>()
	{
		let vals = [0x0123456789abcdefu64 & LsbMask(W), LsbMask(W), 1];
		for v in vals
		{
			let mut a = [0x5555555555555555u64; 2];
			let mut b = a;
			unsafe
			{
				BASetElemConstExpr::<S, W>(a.as_mut_ptr(), v);
				BASetElem(b.as_mut_ptr(), S as u64, W, v);
			}
			assert_eq!(a, b, "{} {}", S, W);
			assert_eq!(unsafe { BAGetElemConstExpr::<S, W>(a.as_ptr()) }, v);
			assert_eq!(unsafe { BAGetElemConstExpr::<S, W>(a.as_ptr()) }, unsafe { BAGetElem(a.as_ptr(), S as u64, W) });
		}
		let mut a = [0u64; 2];
		assert_eq!(unsafe { BAIncrAndGetConstExpr::<S, W>(a.as_mut_ptr(), 1) }, 1);
		assert_eq!(unsafe { BAIncrAndGetConstExpr::<S, W>(a.as_mut_ptr(), 2) }, 3);
	}
	Check::<0, 1>();
	Check::<0, 64>();
	Check::<0, 9>();
	Check::<5, 7>();
	Check::<20, 44>();
	Check::<60, 4>();
	Check::<60, 10>();
	Check::<63, 64>();
	Check::<33, 31>();
}

#[test]
fn bit_array_single_elem_width()
{
	// setUp sets the width of a dynamic-width array (ChangeElemWidth doesn't compile for the compile-time widths, as in C++); a dynamic-width array is returned for further checks
	fn Check<const W: u8>(width: u8, setUp: impl FnOnce(&mut CBitArraySingleElemWidth<W>)) -> CBitArraySingleElemWidth<W>
	{
		let mut a = CBitArraySingleElemWidth::<W>::default();
		setUp(&mut a);
		assert_eq!((a.ElemWidth(), a.ElemsNum()), (width, 0));
		let mask = LsbMask(width);
		let Val = |i: u64| (i.wrapping_mul(0x9E3779B97F4A7C15) >> 3) & mask;
		for i in 0..300u64
		{
			a.Push(Val(i));
		}
		assert_eq!(a.ElemsNum(), 300);
		for i in 0..300u64
		{
			assert_eq!(a.Get(i), Val(i), "W {} width {} i {}", W, width, i);
		}
		// Set only ORs the bits in (as in the C++ version), so it's applied to zeroed elements
		a.SetAllElemsTo0();
		for i in 0..300u64
		{
			assert_eq!(a.Get(i), 0);
			a.Set(i, Val(i + 1));
		}
		for i in 0..300u64
		{
			assert_eq!(a.Get(i), Val(i + 1));
		}
		// Beyond the existing elements: the array grows tightly, and the new elements are 0
		a.ForceElemSetNonExistingTo0(1000, mask);
		assert_eq!((a.ElemsNum(), a.Get(1000), a.Get(999), a.Get(500)), (1001, mask, 0, 0));
		// Writing an existing element ORs the bits in, but for the compile-time width 64, where the word is assigned (as in the C++ version)
		a.ForceElemSetNonExistingTo0(7, 0);
		assert_eq!((a.ElemsNum(), a.Get(7)), (1001, if W == 64 { 0 } else { Val(8) }), "W {} width {}", W, width);
		a
	}
	for width in [1u8, 2, 3, 5, 7, 8, 13, 16, 31, 32, 33, 63, 64]
	{
		let mut a = Check::<0>(width, |a| a.ChangeElemWidth(width));
		// Changing the width of an allocated array only changes the interpretation of the bits
		a.ChangeElemWidth(64);
		assert_eq!(a.ElemWidth(), 64);
	}
	Check::<1>(1, |_| ());
	Check::<2>(2, |_| ());
	Check::<4>(4, |_| ());
	Check::<8>(8, |_| ());
	Check::<16>(16, |_| ());
	Check::<32>(32, |_| ());
	Check::<64>(64, |_| ());
	Check::<7>(7, |_| ());
	Check::<26>(26, |_| ());
	// BitNum: WordInd uses it only for the widths, which aren't compile-time powers of 2 (as in the C++ version), so the other branch is checked directly
	assert_eq!(CBitArraySingleElemWidth::<8>::new().BitNum(5), 40);
	assert_eq!(CBitArraySingleElemWidth::<7>::new().BitNum(5), 35);
	let mut z = CBitArraySingleElemWidth::<0>::new();
	z.SetAllElemsTo0();
	z.ChangeElemWidth(5);
	assert_eq!(z.ElemWidth(), 5);
	for badWidth in [0u8, 65]
	{
		let r = std::panic::catch_unwind(move || CBitArraySingleElemWidth::<0>::new().ChangeElemWidth(badWidth));
		assert_eq!(*r.unwrap_err().downcast::<&str>().unwrap(), "ChangeElemWidth failed, since the new width == 0 or > 64");
	}
	// bad_alloc: on the first allocation and on a reallocation
	let r = std::panic::catch_unwind(|| with_alloc_failure(0, || CBitArraySingleElemWidth::<8>::new().Push(1)));
	alloc_fault::never_fail();
	assert_eq!(*r.unwrap_err().downcast::<&str>().unwrap(), "bad_alloc");
	let r = std::panic::catch_unwind(||
	{
		let mut a = CBitArraySingleElemWidth::<8>::new();
		a.Push(1);
		with_alloc_failure(0, || a.ForceElemSetNonExistingTo0(10000, 1));
	});
	alloc_fault::never_fail();
	assert_eq!(*r.unwrap_err().downcast::<&str>().unwrap(), "bad_alloc");
}

#[test]
fn bit_array_aligned()
{
	fn Check<const W: u8>()
	where
		crate::topor_bit_array_buffer::TElemWidth<W>: crate::topor_bit_array_buffer::TSelAlignedUInt,
	{
		let mut a = CBitArrayAligned::<W>::default();
		assert_eq!((a.ElemWidth(), a.ElemsNum()), (W, 0));
		let mask = LsbMask(W);
		for i in 0..100u64
		{
			a.Push(i.wrapping_mul(0x9E3779B97F4A7C15) & mask);
		}
		assert_eq!(a.ElemsNum(), 100);
		for i in 0..100u64
		{
			assert_eq!(a.Get(i), i.wrapping_mul(0x9E3779B97F4A7C15) & mask);
		}
		a.Set(3, mask);
		assert_eq!(a.Get(3), mask);
		a.ForceElemSetNonExistingTo0(200, 1);
		assert_eq!((a.ElemsNum(), a.Get(200), a.Get(150)), (201, 1, 0));
		a.ForceElemSetNonExistingTo0(5, 0);
		assert_eq!(a.Get(5), 0);
		a.SetAllElemsTo0();
		assert!((0..201).all(|i| a.Get(i) == 0));
	}
	Check::<1>();
	Check::<5>();
	Check::<8>();
	Check::<12>();
	Check::<16>();
	Check::<20>();
	Check::<32>();
	Check::<40>();
	Check::<64>();
	// The element is converted to the aligned type as in C++: a bool is any non-0 value; a narrower integer is truncated
	let mut b = CBitArrayAligned::<1>::new();
	b.Push(6);
	assert_eq!(b.Get(0), 1);
	let mut n = CBitArrayAligned::<12>::new();
	n.Push(0x12345);
	assert_eq!(n.Get(0), 0x2345);
}

// The buffers of the non-contagious bit-array functions and classes must be byte-identical to those of the C++ version:
// tests/data/bit_array_buffer_cpp_golden.txt is the output of the C++ harness tests/data/bit_array_buffer_cpp_golden.cc over ToporBitArrayBuffer.hpp
// (to regenerate: clang++ -std=c++20 -I<repository root> bit_array_buffer_cpp_golden.cc -o golden && ./golden > bit_array_buffer_cpp_golden.txt)
#[test]
fn bit_array_buffer_matches_cpp()
{
	use std::fmt::Write;
	fn Val(i: u64, w: u8) -> u64
	{
		(i.wrapping_mul(0x9E3779B97F4A7C15) >> 3) & LsbMask(w)
	}
	fn Single<const W: u8>(out: &mut String, width: u8, setUp: impl FnOnce(&mut CBitArraySingleElemWidth<W>))
	{
		let mut a = CBitArraySingleElemWidth::<W>::new();
		setUp(&mut a);
		for i in 0..300
		{
			a.Push(Val(i, width));
		}
		write!(out, "S {} {} n={}", W, width, a.ElemsNum()).unwrap();
		for i in 0..300
		{
			write!(out, " {:x}", a.Get(i)).unwrap();
		}
		a.ForceElemSetNonExistingTo0(1000, LsbMask(width));
		writeln!(out, " | {:x} {:x}", a.Get(1000), a.Get(999)).unwrap();
	}
	fn Aligned<const W: u8>(out: &mut String)
	where
		crate::topor_bit_array_buffer::TElemWidth<W>: crate::topor_bit_array_buffer::TSelAlignedUInt,
	{
		let mut a = CBitArrayAligned::<W>::new();
		for i in 0..100u64
		{
			a.Push(i.wrapping_mul(0x9E3779B97F4A7C15));
		}
		write!(out, "A {}", W).unwrap();
		for i in 0..100
		{
			write!(out, " {:x}", a.Get(i)).unwrap();
		}
		writeln!(out).unwrap();
	}
	let mut out = String::new();
	for w in 1..=64u8
	{
		for s in 0..64u64
		{
			let mut buf = [0xaaaaaaaaaaaaaaaau64; 5];
			let mut b = s;
			unsafe
			{
				BASetElemAndGoToNext(buf.as_mut_ptr(), &mut b, w, 0x0123456789abcdef & LsbMask(w));
				BASetElemAndGoToNext(buf.as_mut_ptr(), &mut b, w, 0xfedcba9876543210 & LsbMask(w));
			}
			let inc = unsafe { BAIncrAndGet(buf.as_mut_ptr(), s, w, 3) };
			write!(out, "B {} {}", w, s).unwrap();
			for x in buf
			{
				write!(out, " {:x}", x).unwrap();
			}
			writeln!(out, " {:x} {:x}", unsafe { BAGetElem(buf.as_ptr(), s + w as u64, w) }, inc).unwrap();
		}
	}
	let mut c = [0x5555555555555555u64; 2];
	unsafe { BASetElemConstExpr::<60, 10>(c.as_mut_ptr(), 0x3ff) };
	writeln!(out, "C {:x} {:x} {:x}", c[0], c[1], unsafe { BAGetElemConstExpr::<60, 10>(c.as_ptr()) }).unwrap();
	unsafe { BASetElemConstExpr::<0, 64>(c.as_mut_ptr(), 7) };
	// The C++ evaluates the arguments of printf in an unspecified order; clang evaluates them left to right, as here
	let (c0, c1) = (c[0], c[1]);
	writeln!(out, "C {:x} {:x} {:x}", c0, c1, unsafe { BAIncrAndGetConstExpr::<0, 64>(c.as_mut_ptr(), 2) }).unwrap();
	unsafe { BASetElemConstExpr::<5, 7>(c.as_mut_ptr(), 0x55) };
	writeln!(out, "C {:x} {:x} {:x}", c[0], c[1], unsafe { BAGetElemConstExpr::<5, 7>(c.as_ptr()) }).unwrap();
	for w in [1u8, 2, 3, 5, 7, 8, 13, 16, 31, 32, 33, 63, 64]
	{
		Single::<0>(&mut out, w, |a| a.ChangeElemWidth(w));
	}
	Single::<1>(&mut out, 1, |_| ());
	Single::<2>(&mut out, 2, |_| ());
	Single::<4>(&mut out, 4, |_| ());
	Single::<8>(&mut out, 8, |_| ());
	Single::<16>(&mut out, 16, |_| ());
	Single::<32>(&mut out, 32, |_| ());
	Single::<64>(&mut out, 64, |_| ());
	Single::<7>(&mut out, 7, |_| ());
	Single::<26>(&mut out, 26, |_| ());
	Aligned::<1>(&mut out);
	Aligned::<5>(&mut out);
	Aligned::<8>(&mut out);
	Aligned::<12>(&mut out);
	Aligned::<16>(&mut out);
	Aligned::<20>(&mut out);
	Aligned::<32>(&mut out);
	Aligned::<40>(&mut out);
	Aligned::<64>(&mut out);
	let golden = include_str!("data/bit_array_buffer_cpp_golden.txt");
	for (i, (r, c)) in out.lines().zip(golden.lines()).enumerate()
	{
		assert_eq!(r, c, "line {}", i + 1);
	}
	assert_eq!(out.lines().count(), golden.lines().count());
}
