// Fixture for the DXIL lifter: branches that are not plain diamonds (short-circuit tests, an early return, a discard,
// a chain of ifs sharing an else). Compile: dxc -T ps_6_0 -E main -Fo flow.dxil flow.hlsl
cbuffer Values : register(b0)
{
    float4 a;
    int4 n;
};
struct Output
{
    float4 x : SV_Target0;
    uint4 k : SV_Target1;
};
Output main(float4 position : SV_Position, float2 t : TEXCOORD0)
{
    Output o;
    o.x = float4(0, 0, 0, 1);
    o.k = uint4(0, 0, 0, 0);
    // a && b ? X : Y (the else is reached from two tests)
    if (t.x > a.x && t.y < a.y)
        o.x.x = a.z * t.x;
    else
        o.x.x = a.w - t.y;
    // a || b
    if (n.x > 3 || n.y < 2)
        o.k.x = 5;
    else
        o.k.x = 9;
    // an early return
    if (n.z == 7)
    {
        o.x.y = 1.0;
        return o;
    }
    // a discard under a condition
    if (t.x < -1.0)
        discard;
    // a nested if whose inner else joins the outer's end
    if (n.w > 0)
    {
        if (t.y > 0.5)
            o.k.y = 11;
        else
            o.k.y = 13;
    }
    else
    {
        o.k.y = 17;
    }
    o.x.z = (n.x > n.y) ? a.x : a.y;
    return o;
}
