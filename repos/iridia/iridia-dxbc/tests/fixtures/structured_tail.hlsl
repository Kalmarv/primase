struct Tail { double wide; float narrow; };
StructuredBuffer<Tail> entries : register(t0);
cbuffer Control { uint index; };
float4 main() : SV_Target0 { return entries[index].narrow + float(entries[index].wide); }
