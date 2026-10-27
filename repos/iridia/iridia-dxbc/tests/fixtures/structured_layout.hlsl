// Original layout oracle. Windows SDK 10.0.26100.0 fxc ps_5_0 and dxc ps_6_0, entry main.
// Structured packing differs from cbuffer packing: scalar arrays and matrix lines are tight.
struct Nested { float3 direction; float weight; };
struct Entry {
    float scalar;
    float values[3];
    row_major float2x3 rows;
    column_major float2x3 columns;
    Nested nested[2];
};
StructuredBuffer<Entry> entries : register(t3);
StructuredBuffer<uint> indices : register(t4);
StructuredBuffer<uint3> lookup : register(t5);
cbuffer Control : register(b0) { uint index; };
float4 main() : SV_Target0 {
    Entry e = entries[indices[index]];
    return float4(e.scalar + e.values[2], e.rows[1][2], e.columns[1][2],
                  e.nested[1].weight + lookup[index].z);
}
