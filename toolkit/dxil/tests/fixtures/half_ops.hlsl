// 16-bit floats and integers (pixel). Compiled with dxc -T ps_6_2 -enable-16bit-types -E main (half_ops.dxil, also
// -Od). With a = 1.0009765625 (1 + 2^-10, a half) and b = 1.0:
//   x = (half)a, y = (half)b; x + y = 2.0009765625 is half-way between the halves 2.0 and 2.001953125: it rounds to
//   the even one, 2.0; x * y = 1.0009765625; n = (uint16_t)b - (uint16_t)(b + b) wraps to 65535;
//   (half)(1.0 / 3.0) is the half 0.333251953125.
// SV_Target0 = (2.0, 1.0009765625, 65535.0, 0.333251953125) for cbuffer (a, b) = (1.0009765625, 1.0).
cbuffer Values : register(b0) {
    float a;
    float b;
};

float4 main(float4 position : SV_Position) : SV_Target0 {
    half x = (half)a;
    half y = (half)b;
    half sum = x + y;
    half product = x * y;
    uint16_t n = (uint16_t)b - (uint16_t)(b + b);
    half third = (half)(1.0 / 3.0 + 0.0 * a);
    return float4(sum, product, n, third);
}
