// Unit tests: the types, constants and static helpers of the solver class (topi_types.rs)

use super::containers::with_alloc_failure;
use crate::topi_types::*;
use crate::topor_bit_array::CBitArray;
use crate::topor_dyn_array::Span;

#[test]
fn literal_helpers()
{
	assert_eq!(GetVar(9), 4);
	assert_eq!(GetLit(4, true), 9);
	assert_eq!(GetFirstLit(), 2);
	assert!(IsPos(8) && IsNeg(9));
	assert_eq!(Negate(8), 9);
	assert_eq!(GetMaxLit(8), 9);
	assert_eq!(GetMaxLit(9), 9);
	assert_eq!(ExternalLit2ExternalVar(-5), 5);
	assert_eq!(ExternalLit2ExternalVar(5), 5);
	assert_eq!(HexStr(255u32, 8), "000000ff");
	assert_eq!(HexStrInd(255u64), "00000000000000ff");
	assert_eq!(HexStrInd(255u32), "000000ff");
	assert_eq!(memMbVec(&[0u64; 1000]), 8);
}

#[test]
fn tuind()
{
	fn check<Ind: TUInd>(lits: usize)
	{
		assert_eq!(Ind::LitsInInd, lits);
		assert_eq!(Ind::from_usize(5).as_usize(), 5);
		assert_eq!(Ind::from_u64(6).as_u64(), 6);
		assert_eq!(Ind::from_u32(7).as_u32(), 7);
		assert_eq!(Ind::ZERO + Ind::ONE, Ind::ONE);
		let mut buf = [0 as TULit; 3];
		unsafe { Ind::from_u64(0x1_0000_0002 & Ind::MAX.as_u64()).write(buf.as_mut_ptr().add(1)) };
		assert_eq!(buf[1], 2);
		assert_eq!(unsafe { Ind::read(buf.as_ptr().add(1)) }.as_u64(), 0x1_0000_0002 & Ind::MAX.as_u64());
		assert_eq!(TWatchInfo::<Ind>::BinsInLong, 1 + lits);
		assert_eq!(TWatchInfo::<Ind>::BinsInLongBitCeil, 2 * lits);
		assert_eq!(TWatchInfo::<Ind>::BinsInLongBytes, 4 * (1 + lits));
		assert_eq!(TWatchInfo::<Ind>::MaxWatchInfoAlloc().as_u64(), 1u64 << (Ind::DIGITS - 1));
		let mut wi = TWatchInfo::<Ind>::default();
		assert!(wi.IsEmpty());
		wi.PointToNewArena(Ind::from_u32(16), Ind::from_u32(8));
		assert!(!wi.IsEmpty());
		wi.m_LongWatches = Ind::from_u32(2);
		wi.m_BinaryWatches = Ind::from_u32(1);
		assert_eq!(wi.GetLongEntries(), 2 * (1 + lits));
		assert_eq!(wi.GetUsedEntries(), 2 * (1 + lits) + 1);
		assert_eq!(wi.GetLongEntry(1), 1 + lits);
		assert_eq!(wi.GetBinEntry(0), 2 * (1 + lits));
	}
	check::<u32>(1);
	check::<u64>(2);
}

#[test]
fn span_helpers()
{
	let mut v: Vec<TULit> = vec![4, 2, 9, 2, 7];
	let s = Span::from_slice(&mut v);
	assert_eq!(TULitSpan::size(&s), 5);
	assert_eq!(TULitSpan::back(&s), 7);
	assert_eq!(TULitSpan::find(&s, 9), 2);
	assert_eq!(TULitSpan::find(&s, 99), 5);
	assert_eq!(s.lits().collect::<Vec<_>>(), vec![4, 2, 9, 2, 7]);
	// The first minimal/maximal element
	assert_eq!(min_element(s, 0, 5, |l| l), 1);
	assert_eq!(max_element(s, 0, 5, |l| l), 2);
	assert_eq!(max_element(s, 3, 5, |l| l), 4);
	assert_eq!(min_element(s, 2, 2, |l| l), 2);
	assert_eq!(max_element(s, 5, 5, |l| l), 5);
	assert_eq!(std_min_element(s, 0, 5, |a, b| a < b), 1);
	assert_eq!(std_min_element(s, 4, 4, |a, b| a < b), 4);
	// move_if moves the elements, for which the predicate doesn't hold, to the front
	let newEnd = move_if(s, 0, 5, |l| l == 2);
	assert_eq!(newEnd, 3);
	assert_eq!(s.subspan(0, 3).to_vec(), vec![4, 9, 7]);
	assert_eq!(move_if(s, 0, 3, |l| l == 100), 3);
	TULitSpan::swap(&s, 0, 1);
	TULitSpan::set_back(&s, 1);
	assert_eq!(v, vec![9, 4, 7, 2, 1]);
}

#[test]
fn bc_hash_id_and_ind()
{
	let h = TBCHashId::new(true, 3, 17);
	assert!(h.m_IsLearnt());
	assert_eq!((h.m_BitsForClsSize(), h.m_BitsForLit()), (3, 17));
	assert_eq!(h.val(), 1 | (3 << 1) | (17 << 6));
	assert_eq!(TBCHashId::from_val(h.val()), h);
	assert_eq!(TBCHashId::BCHashIdBits(), 11);
	assert!(!h.IsError());
	assert_eq!(h.GetBitsGlue(), 5);
	assert_eq!(h.MaxGlue(), 31);
	assert_eq!(h.GetFirstLitOffset(), 3 + 5 + 32);
	let init = TBCHashId::new(false, 3, 17);
	assert_eq!(init.GetFirstLitOffset(), 3);
	assert_eq!(TBCHashId::new(true, 10, 20).GetBitsGlue(), 11);
	let mut e = h;
	e.SetError();
	assert!(e.IsError() && TBCHashId::default().IsError());
	assert_eq!((TBCHashId::GetBitsSkipDel(), TBCHashId::GetBitsActivity(), TBCHashId::GetBitsActivityAndSkipDel()), (1, 31, 32));

	let i = TBCInd::<u64>::new(h, 1000);
	assert!(!i.IsError());
	assert!(i.m_IsLearnt());
	assert_eq!((i.m_BitsForClsSize(), i.m_BitsForLit(), i.m_BitStart()), (3, 17, 1000));
	assert_eq!(i.GetHashId(), h);
	assert_eq!(i.BitFirstLit(), 1040);
	assert_eq!(i.GetBitsGlue(), 5);
	assert_eq!(TBCInd::<u64>::from_ind(i.ind()), i);
	// An erroneous hash-id or a bit-start, which doesn't fit, result in an erroneous index
	assert!(TBCInd::<u64>::new(e, 1000).IsError());
	assert!(TBCInd::<u64>::new(h, TBCInd::<u64>::m_MaxBitStart + 1).IsError());
	assert!(!TBCInd::<u64>::new(h, TBCInd::<u64>::m_MaxBitStart).IsError());
	assert!(TBCInd::<u32>::new(h, 1 << 21).IsError());
	assert_eq!(TBCInd::<u32>::BitsForBCParentClsInd, 21);

	assert_eq!(BCClsSize2Bits(60), 5);
	assert_eq!(BCClsSize2Bits(61), 6);
	assert_eq!(BCClsSize2Bits(u32::MAX as u64), 31);
	for sz in 3..2000u64
	{
		assert_eq!(BCEncodedClsSize2ClsSizeConst(BCClsSize2EncodedClsSize(sz), BCClsSize2Bits(sz)) as u64, sz);
		assert!(sz == 3 || (BCClsSize2EncodedClsSize(sz) as u64) < (1 << BCClsSize2Bits(sz)));
	}
}

#[test]
fn compressed_clss_table()
{
	let mut t = TCompressedClss::default();
	assert_eq!((t.size(), t.begin(), t.end()), (0, BCTableSize, BCTableSize));
	assert_eq!(t.memMb(), 0);
	let ba = t.insert(100);
	assert!(!ba.is_null());
	assert_eq!(t.insert(100), ba);
	assert!(!t.insert(7).is_null());
	assert!(!t.insert(2047).is_null());
	assert_eq!(t.size(), 3);
	assert!(t.exists(7) && !t.exists(8));
	assert_eq!(t.at(100), ba);
	unsafe { (*ba).bit_reserve_new_chunk(64 * 1000, 0) };
	assert!(t.memMb() >= 8 + 6);
	let mut ids = vec![];
	let mut it = t.begin();
	while it != t.end()
	{
		ids.push(it);
		it = t.next(it);
	}
	assert_eq!(ids, vec![7, 100, 2047]);
	assert_eq!(t.lower_bound(8), 100);
	t.erase(100);
	t.erase(100);
	assert_eq!(t.size(), 2);
	assert!(with_alloc_failure(0, || t.insert(55)).is_null());
	assert_eq!(t.size(), 2);
}

#[test]
fn cls_views()
{
	// Standard
	let mut lits: Vec<TULit> = vec![10, 11, 12, 13];
	let c = CCls::<u32, false>::new_standard(5, Span::from_slice(&mut lits));
	assert_eq!((c.cls_ind(), c.size(), c.get(2), c.back()), (5, 4, 12, 13));
	c.set(0, 20);
	c.swap(0, 3);
	assert_eq!(c.span().to_vec(), vec![13, 11, 12, 20]);
	assert_eq!(c.find(12), 2);
	let c2 = c.clone();
	assert_eq!(c2.lits().sum::<u32>(), 56);

	// Compressed: [size:2][l:5]x6 for an initial clause of 6 literals
	let mut ba = CBitArray::new();
	ba.bit_reserve_new_chunk(200, 0);
	let h = TBCHashId::new(false, BCClsSize2Bits(6), 5);
	ba.bit_push(17, 9);
	let bitStart = ba.bit_get_next_bit();
	ba.bit_push(BCClsSize2EncodedClsSize(6) as u64, h.m_BitsForClsSize());
	for l in [3u64, 5, 7, 9, 11, 13]
	{
		ba.bit_push(l, 5);
	}
	let bcInd = TBCInd::<u64>::new(h, bitStart);
	let cc = CCls::<u64, true>::new_compressed(&mut ba, bcInd);
	assert_eq!((cc.size(), cc.get(0), cc.get(5), cc.cls_ind()), (6, 3, 13, bcInd.ind()));
	cc.set(1, 31);
	cc.swap(0, 5);
	cc.set_back(2);
	assert_eq!(cc.lits().collect::<Vec<_>>(), vec![13, 31, 7, 9, 11, 2]);
	assert_eq!(ba.bit_get(0, 9), 17);
	// A ternary clause has no size field
	let h3 = TBCHashId::new(false, 0, 4);
	let bitStart3 = ba.bit_get_next_bit();
	for l in [2u64, 4, 6]
	{
		ba.bit_push(l, 4);
	}
	let c3 = CCls::<u64, true>::new_compressed(&mut ba, TBCInd::new(h3, bitStart3));
	assert_eq!(c3.lits().collect::<Vec<_>>(), vec![2, 4, 6]);

	// Snapshots
	let s = CClsConstSnapshotDense::new_standard(Span::from_slice(&mut lits));
	assert_eq!((s.size(), s.get(0), s.back(), s.iter().count(), s.span().size()), (4, 13, 20, 4, 4));
	let s = CClsConstSnapshotDense::new_compressed(vec![1, 2, 3]);
	assert_eq!((s.size(), s.get(1), s.back()), (3, 2, 3));
}

#[test]
fn small_structs()
{
	let mut ai = TAssignmentInfo::default();
	ai.Assign(true, 0u32, 6);
	assert!(ai.m_IsAssigned() && ai.IsAssignedBinary() && ai.m_IsNegated());
	ai.Assign(false, 5u32, 6);
	assert!(ai.m_IsAssigned() && !ai.IsAssignedBinary() && !ai.m_IsNegated());
	ai.Assign(false, 0u32, BadULit);
	assert!(!ai.IsAssignedBinary());
	ai.set_m_Visit(true);
	ai.set_m_Root(true);
	ai.set_m_IsAssump(true);
	ai.set_m_IsAssumpNegated(true);
	assert!(ai.m_Visit() && ai.m_Root() && ai.m_IsAssump() && ai.m_IsAssumpNegated());
	ai.Unassign();
	ai.set_m_Visit(false);
	assert!(!ai.m_IsAssigned() && !ai.m_Visit() && ai.m_Root());

	let mut vi = TVarInfo::<u64>::default();
	vi.Assign(0, 6, 3, 1, 2);
	assert_eq!((vi.m_BinOtherLit(), vi.m_DecLevel, vi.m_TrailPrev, vi.m_TrailNext), (6, 3, 1, 2));
	assert!(!vi.IsDecVar());
	assert_eq!(unsafe { *vi.m_BinOtherLitPtr() }, 6);
	vi.Assign(0, BadULit, 3, 1, 2);
	assert!(vi.IsDecVar());
	vi.Assign(0, BadULit, 0, 1, 2);
	assert!(!vi.IsDecVar());
	vi.Assign(77, 6, 3, 1, 2);
	assert_eq!(vi.m_ParentClsInd, 77);

	let mut pi = TPolarityInfo::new(false, true);
	assert!(pi.IsNextPolarityDetermined() && pi.m_IsNextPolarityNegated() && !pi.m_IsPolarityFixed());
	assert!(pi.GetNextPolarityIsNegated());
	assert!(!pi.IsNextPolarityDetermined());
	let mut pi = TPolarityInfo::new(true, false);
	assert!(!pi.GetNextPolarityIsNegated());
	assert!(pi.IsNextPolarityDetermined());
	pi.Clear();
	assert_eq!(pi.m_Bits, 0);

	let ci = TContradictionInfo::<u32>::new();
	assert!(!ci.IsContradiction());
	let ci = TContradictionInfo::<u32>::from_parent(9);
	assert!(ci.IsContradiction() && !ci.m_IsContradictionInBinaryCls && ci.m_ParentClsInd == 9);
	let ci = TContradictionInfo::<u32>::from_bin([2, 5]);
	assert!(ci.IsContradiction() && ci.m_IsContradictionInBinaryCls && ci.m_BinClause == [2, 5]);

	let di = TDelImpl::<u32>::new(2, 4, 6);
	assert_eq!((di.m_L, di.m_OtherWatch, di.m_ParentClsInd), (2, 4, 6));
	let ps = TParentSubsumed::<u64>::new(2, true, 8);
	assert_eq!((ps.m_L, ps.m_IsBinary, ps.m_BinOtherLit()), (2, true, 8));

	let mut di = TClsDelInfo { m_GlueNeverDelete: 2, m_Clusters: 11, m_MaxClusteredGlue: 16, ..Default::default() };
	*di.m_CurrChange() = 5;
	assert_eq!(di.m_TriggerMax, 5);
	assert_eq!(di.GetCluster(17), u8::MAX);
	assert_eq!(di.GetCluster(3), 0);
	assert_eq!(di.GetCluster(16), 13);
	di.m_Clusters = 3;
	assert_eq!(di.GetCluster(16), 3);
	di.m_Clusters = 100;
	assert_eq!(di.GetCluster(4), 1);
}

#[test]
fn handle_new_cls()
{
	let mut h = CHandleNewCls::new(4);
	h.SetMultiplier(2.);
	h.NewClause();
	assert_eq!(h.NewLitIsTauIsDuplicate(GetLit(1, false)), (false, false, false));
	assert_eq!(h.NewLitIsTauIsDuplicate(GetLit(2, true)), (false, false, false));
	assert_eq!(h.NewLitIsTauIsDuplicate(GetLit(1, false)), (false, true, false));
	assert_eq!(h.NewLitIsTauIsDuplicate(GetLit(2, true)), (false, true, false));
	assert_eq!(h.NewLitIsTauIsDuplicate(GetLit(2, false)), (true, false, false));
	// A variable beyond the capacity
	assert_eq!(h.NewLitIsTauIsDuplicate(GetLit(100, true)), (false, false, false));
	assert_eq!(h.NewLitIsTauIsDuplicate(GetLit(100, false)), (true, false, false));
	assert_eq!(h.GetCurrCls().to_vec(), vec![GetLit(1, false), GetLit(2, true), GetLit(100, true)]);
	h.NewClause();
	assert!(h.GetCurrCls().is_empty());
	assert_eq!(h.NewLitIsTauIsDuplicate(GetLit(2, false)), (false, false, false));
	let _ = h.memMb();
	h.reserve_exactly(300);
	h.reserve_exactly_init(400, 0);
	assert!(!h.uninitialized_or_erroneous());

	// Allocation failures: the counters, then the clause
	let mut h = CHandleNewCls::new(4);
	h.NewClause();
	assert_eq!(with_alloc_failure(0, || h.NewLitIsTauIsDuplicate(GetLit(100, true))), (false, false, true));
	let mut h = CHandleNewCls::new(4);
	h.NewClause();
	assert_eq!(with_alloc_failure(0, || h.NewLitIsTauIsDuplicate(GetLit(1, true))), (false, false, true));

	// The counter wraps around
	let mut h = CHandleNewCls::new(4);
	for _ in 0..3
	{
		h.NewClause();
	}
	assert_eq!(h.NewLitIsTauIsDuplicate(GetLit(1, false)), (false, false, false));
	h.SetCounterForTest(i32::MAX);
	h.NewClause();
	assert_eq!(h.NewLitIsTauIsDuplicate(GetLit(1, true)), (false, false, false));
	assert_eq!(h.NewLitIsTauIsDuplicate(GetLit(1, true)), (false, true, false));
}
