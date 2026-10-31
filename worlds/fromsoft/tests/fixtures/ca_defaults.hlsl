// Synthetic CA defaults fixture: reflected component order differs from metadata order,
// with padding, a nonzero structure base, and an independent material array.
// fxc /T ps_5_0 /E main /Fo ca_defaults.dxbc ca_defaults.hlsl
struct PerDraw
{
    float M_Test_snp_0_color_0_2;
    float spare;
    float M_Test_snp_0_color_0_0;
    float M_Test_snp_0_color_0_3;
    float M_Test_snp_0_color_0_1;
};
struct MaterialValues
{
    float M_Test_snp_array_0;
};
cbuffer Material : register(b2)
{
    float4 prefix;
    PerDraw SAT_CAParam;
    MaterialValues SAT_MtdParam;
};
float4 main() : SV_Target2
{
    return float4(SAT_CAParam.M_Test_snp_0_color_0_0,
                  SAT_CAParam.M_Test_snp_0_color_0_1,
                  SAT_CAParam.M_Test_snp_0_color_0_2,
                  SAT_CAParam.M_Test_snp_0_color_0_3) * SAT_MtdParam.M_Test_snp_array_0;
}
