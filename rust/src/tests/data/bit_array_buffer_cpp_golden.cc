#include "ToporBitArrayBuffer.hpp"
#include <cstdio>
using namespace BitArray;
static uint64_t Mask(uint8_t w) { return w == 64 ? ~0ull : (1ull << w) - 1; }
static uint64_t Val(uint64_t i, uint8_t w) { return ((i * 0x9E3779B97F4A7C15ull) >> 3) & Mask(w); }
template <uint8_t W> void Single(uint8_t width)
{
	CBitArraySingleElemWidth<W> a;
	if constexpr (W == 0) a.ChangeElemWidth(width);
	for (uint64_t i = 0; i < 300; ++i) a.Push(Val(i, width));
	printf("S %u %u n=%llu", (unsigned)W, (unsigned)width, (unsigned long long)a.ElemsNum());
	for (uint64_t i = 0; i < 300; ++i) printf(" %llx", (unsigned long long)a.Get(i));
	a.ForceElemSetNonExistingTo0(1000, Mask(width));
	printf(" | %llx %llx\n", (unsigned long long)a.Get(1000), (unsigned long long)a.Get(999));
}
template <uint8_t W> void Aligned()
{
	CBitArrayAligned<W> a;
	for (uint64_t i = 0; i < 100; ++i) a.Push(i * 0x9E3779B97F4A7C15ull);
	printf("A %u", (unsigned)W);
	for (uint64_t i = 0; i < 100; ++i) printf(" %llx", (unsigned long long)a.Get(i));
	printf("\n");
}
int main()
{
	for (int w = 1; w <= 64; ++w)
		for (int s = 0; s < 64; ++s)
		{
			uint64_t buf[5]; for (auto& x : buf) x = 0xaaaaaaaaaaaaaaaaull;
			uint64_t b = s;
			BASetElemAndGoToNext(buf, b, w, 0x0123456789abcdefull & Mask(w));
			BASetElemAndGoToNext(buf, b, w, 0xfedcba9876543210ull & Mask(w));
			uint64_t inc = BAIncrAndGet(buf, s, w, 3);
			printf("B %d %d", w, s);
			for (auto x : buf) printf(" %llx", (unsigned long long)x);
			printf(" %llx %llx\n", (unsigned long long)BAGetElem(buf, s + w, w), (unsigned long long)inc);
		}
	uint64_t c[2] = {0x5555555555555555ull, 0x5555555555555555ull};
	BASetElem<60, 10>(c, 0x3ff); printf("C %llx %llx %llx\n", (unsigned long long)c[0], (unsigned long long)c[1], (unsigned long long)BAGetElem<60, 10>(c));
	BASetElem<0, 64>(c, 7); printf("C %llx %llx %llx\n", (unsigned long long)c[0], (unsigned long long)c[1], (unsigned long long)BAIncrAndGet<0, 64>(c, 2));
	BASetElem<5, 7>(c, 0x55); printf("C %llx %llx %llx\n", (unsigned long long)c[0], (unsigned long long)c[1], (unsigned long long)BAGetElem<5, 7>(c));
	for (int w : {1, 2, 3, 5, 7, 8, 13, 16, 31, 32, 33, 63, 64}) Single<0>(w);
	Single<1>(1); Single<2>(2); Single<4>(4); Single<8>(8); Single<16>(16); Single<32>(32); Single<64>(64); Single<7>(7); Single<26>(26);
	Aligned<1>(); Aligned<5>(); Aligned<8>(); Aligned<12>(); Aligned<16>(); Aligned<20>(); Aligned<32>(); Aligned<40>(); Aligned<64>();
}
