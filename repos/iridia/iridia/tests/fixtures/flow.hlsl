// Fixtures for the dataflow query (tests/flow.rs), one pixel program per entry point, each compiled to both bytecodes:
//   fxc /T ps_5_0 /E <entry> /Fo flow_<entry>.dxbc flow.hlsl
//   dxc -T ps_6_0 -E <entry> -Fo flow_<entry>.dxil flow.hlsl
// (fxc 10.1 and dxc 1.7.2308.16, of the Windows 10 SDK 10.0.26100.0)
// Each declares a vertex colour and reaches the target, or the discard, with it in one way only: its value (`value`),
// as a texel's coordinate (`coordinate`), as the condition of a discard and of a store (`control`), or not at all
// (`never`). `blend` is for the structure: a layer blended over a base by a mask, times another texture. `carried` is a
// loop and a switch the colour decides (fxc writes the switch as branches, dxc as a switch; dxc leaves its loops by a
// flag).
cbuffer Material : register(b0)
{
    float threshold;
    float2 tiling;
    float strength;
};
Texture2D base_map : register(t0);
Texture2D detail_map : register(t1);
Texture2D mask_map : register(t2);
Texture2D tint_map : register(t3);
SamplerState linear_wrap : register(s0);

struct Input
{
    float4 position : SV_Position;
    float4 uv : TEXCOORD0;
    float4 colour : COLOR0;
};

// the vertex colour's alpha multiplies the texel's: COLOR0.w reaches SV_Target0.w by value
float4 value(Input i) : SV_Target0
{
    float4 t = base_map.Sample(linear_wrap, i.uv.xy);
    return float4(t.rgb, t.a * i.colour.a);
}

// the vertex colour only says where the texel is read
float4 coordinate(Input i) : SV_Target0
{
    return base_map.Sample(linear_wrap, i.colour.xy);
}

// the vertex colour's alpha decides whether the pixel is drawn, and its red which of two texels is stored
float4 control(Input i) : SV_Target0
{
    if (i.colour.a < threshold)
        discard;
    float4 t;
    [branch] if (i.colour.r > 0.5)
        t = base_map.Sample(linear_wrap, i.uv.xy);
    else
        t = detail_map.Sample(linear_wrap, i.uv.xy);
    return t;
}

// the vertex colour is declared and not read
float4 never(Input i) : SV_Target0
{
    return base_map.Sample(linear_wrap, i.uv.xy);
}

// a detail layer blended over the base by a mask's alpha, the result times a tint read on the second UV pair
float4 blend(Input i) : SV_Target0
{
    float4 base = base_map.Sample(linear_wrap, i.uv.xy);
    float4 detail = detail_map.Sample(linear_wrap, i.uv.xy * tiling);
    float mask = mask_map.Sample(linear_wrap, i.uv.xy).a;
    float4 tint = tint_map.Sample(linear_wrap, i.uv.zw);
    return lerp(base, base * detail * strength, mask) * tint;
}

// a sum a loop carries, for as many iterations as the vertex colour's blue says, then a case the green chooses
float4 carried(Input i) : SV_Target0
{
    float4 sum = 0;
    [loop] for (int k = 0; k < (int)(i.colour.b * 8); k++)
        sum += base_map.Sample(linear_wrap, i.uv.xy + k * 0.01);
    [branch] switch ((int)i.colour.g)
    {
    case 0:
        sum *= strength;
        break;
    case 1:
        sum += detail_map.Sample(linear_wrap, i.uv.xy);
        break;
    default:
        break;
    }
    return sum;
}
