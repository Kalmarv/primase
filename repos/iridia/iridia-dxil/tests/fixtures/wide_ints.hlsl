// 64-bit integers (compute). Compiled with dxc -T cs_6_0 -E main (wide_ints.dxil, also -Od and -T cs_6_6). The a and b
// of the constant buffer are 64-bit values given as halves; each result is written as its low then its high word:
//   a + b, a - b, a * b, a & b, a | b, a ^ b, a << n, a >> n (logical), (int64) a >> n (arithmetic), a / b, a % b,
//   (int64) a / (int64) b, (int64) a % (int64) b, then a < b, (int64) a < (int64) b, a == b as 0 or 1,
//   then countbits(a), firstbitlow(a), firstbithigh(a)
cbuffer C : register(b0) {
    uint a_lo;
    uint a_hi;
    uint b_lo;
    uint b_hi;
    uint n;
};
RWStructuredBuffer<uint> results : register(u0);

[numthreads(1, 1, 1)]
void main() {
    uint64_t a = ((uint64_t)a_hi << 32) | a_lo;
    uint64_t b = ((uint64_t)b_hi << 32) | b_lo;
    uint at = 0;
#define PUT(v) { uint64_t t = (uint64_t)(v); results[at++] = (uint)t; results[at++] = (uint)(t >> 32); }
    PUT(a + b);
    PUT(a - b);
    PUT(a * b);
    PUT(a & b);
    PUT(a | b);
    PUT(a ^ b);
    PUT(a << n);
    PUT(a >> n);
    PUT((int64_t)a >> n);
    PUT(a / b);
    PUT(a % b);
    PUT((int64_t)a / (int64_t)b);
    PUT((int64_t)a % (int64_t)b);
    results[at++] = a < b;
    results[at++] = (int64_t)a < (int64_t)b;
    results[at++] = a == b;
    results[at++] = countbits(a);
    results[at++] = firstbitlow(a);
    results[at++] = firstbithigh(a);
}
