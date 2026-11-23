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
	let pattern = |i: u64| (i * 2654435761u64 >> 7) & 1;
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
