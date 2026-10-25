//! Surface terms that core.pbr cannot encode. Keep the actual equations and resolved parameter values beside
//! the lowered material, not just a family name. The original programs remain the executable representation.
use super::{Inputs, MaterialBinding, MaterialValue, TextureSource};

/// Stored rates are not proof of an active animation: MaterialValue's eight floats do not retain the operator's
/// activation bytes, and no read consumer establishes the engine-clock conversion. Do not invent units per second.
pub(super) fn uv_motion(f: &[f32]) -> Option<String> {
    let &[su,sv,tu,tv,du,dv,rotation,angular] = f else {return None;};
    if (du,dv,angular)==(0.0,0.0,0.0) {return None;}
    Some(format!("UVTransform stored scale=({su},{sv}), translation=({tu},{tv}), rotation={rotation} radians; \
        TranslationSpeedU={du}, TranslationSpeedV={dv}, RotationSpeed={angular}; core has no UV-animation component; \
        activation bytes and engine-clock/rate conversion are not retained/established, so no t*rate motion is invented"))
}

pub(super) fn notices(i: &Inputs<'_>) -> Vec<String> {
    let template=i.m.template_name.as_deref().unwrap_or("unnamed");
    let mut out=Vec::new();
    let mut push=|text:String|out.push(format!("{template} Material {:#x}: {text}",i.m.index_id));
    let layout=match template {
        "MAD_Tex1"|"MAD_Tex1_SkinningSupported"=>Some(super::TEX1),
        "MAD_Tex1_TerrainBlend"=>Some(super::TEX1_TERRAIN_BLEND),
        "MAD_Tex1_Emissive"=>Some(super::TEX1_EMISSIVE),
        "MAD_Tex1_UGE"=>Some(super::TEX1_UGE),
        "MAD_Tex1_TeamColoured"=>Some(super::TEX1_TEAM_COLOURED),
        _=>None,
    };
    if let Some(layout)=layout {
        let vci=layout.vertex_colour.map_or(0.0,|r|i.scalar(r));
        if vci!=0.0 {
            let alpha=layout.alpha.map_or(1.0,|r|i.scalar(r));
            push(format!("G-buffer B=tintedDiffuse*(1+{vci}*(COLOR0.rgb-1)); coverage A=diffuse.a*{alpha}, \
                independent of COLOR0.a. core's default vertex product multiplies RGBA; it has no RGB-only or \
                interpolated affine colour field. Shaded keeps its stated vertex approximation; it must not be read as shader coverage"));
            if let Some(em)=layout.emission {
                let slot=match em.mask {TextureSource::Bound(s)=>format!("t{}",75+s),TextureSource::Set(s)=>format!("TextureSet[{s}]")};
                push(format!("G-buffer E=B*({slot}[{}]*{}/t23.x), including the RGB vertex factor above; \
                    Shaded E uses tintedDiffuse without that vertex factor (core vertex colour affects base only); \
                    COLOR0.a is not an emission factor; stated emissive-exposure is not a read of t23",em.lane,i.scalar(em.scale)));
            }
        }
    }
    if let Some(rows)=match template {"MAD_Tex2"=>Some(super::TEX2_ROWS),
        "MAD_Tex2_TintBFromShaderConstants"=>Some(super::TEX2_TINT_B_ROWS),_=>None} {
        let vci=rows.vertex_colour.map_or(0.0,|r|i.scalar(r));
        if vci!=0.0 {push(format!("G-buffer B=heightBlend(A,B)*(1+{vci}*(COLOR0.rgb-1)); \
            COLOR0.a supplies sat((a-.5)*{}+.5+{}*{}) to layer B, not coverage or emission. \
            Shaded retains the alpha weight and omits the RGB factor; rewriting COLOR0.a to a colour factor would corrupt that weight",
            i.scalar(rows.alpha_contrast),i.scalar(rows.alpha_contrast),i.scalar(rows.alpha_offset)));}
    }
    match template {
        "MAD_Tex1_Wind"=>push(format!("G-buffer translucency o3.w={} (cb3[18].x), not opacity or emission; \
            core.pbr has no transmission term. Wind uses the retained vertex program's cb3[1..13], not a material UV scroll",i.scalar(18))),
        "MAD_Bark"=>{
            push(format!("layout B: M=0; O=(1+{}*(t78.b-1))*COLOR0.a in o1.w; specular occlusion o2.z=1, \
                translucency o2.w=0. Shaded O omits COLOR0.a and reaches ambient light only",i.scalar(16)));
            wind(i,1,14,17,&mut push);
        }
        "MAD_Tex1_Leaves"=>{
            push(format!("layout B: M=0; B=lerp(tint(t77),t77,p)*sat(COLOR0.a+{}), \
                p=sat({}-frac(frac(.067111*instanceX+.005837*instanceY)*52.982918)); \
                O=(1+{}*(t78.b-1))*COLOR0.a; specular occlusion o2.z=t78.b*{}; translucency o2.w=t78.r*{}. \
                Shaded lacks the instance mix, vertex alpha factors and separate specular/translucency fields",
                i.scalar(25),i.scalar(26),i.scalar(27),i.scalar(21),i.scalar(31)));
            push(format!("coverage A=t79.r (TextureSet slot 18), not diffuse.a or COLOR0.a; cut-out compares \
                A to cb3[0].z. Near fade cb3[24]={}, dead-tree threshold cb3[29]={} with switch cb3[30]={} \
                alter dither/coverage, not PBR roughness. The engine's reference/state producer remains unresolved",
                i.scalar(24),i.scalar(29),i.scalar(30)));
            wind(i,1,20,32,&mut push);
        }
        "MAD_Clutters_Grass"=>{
            push(format!("layout B opaque: M=0; R=t78.a; O=1+{}*(t75.b-1); specular occlusion o2.z={} \
                (including zero); translucency o2.w=t75.r*{}. These are not alpha or metalness; core.pbr has no separate specular occlusion/translucency",
                i.scalar(16),i.scalar(11),i.scalar(10)));
            push(format!("B=t77.rgb+(worldNoise.b*{})*(HSV(t77.rgb,H={},S={},V={})-t77.rgb); \
                worldNoise t79 uses world-space UV, absent from the static texture recipe. Blended permutations instead write \
                A=t76.r to o0.w/o2.w; cut-out/fade uses per-draw reference and t86 instance data, not the opaque R/translucency contract",
                i.scalar(15),i.scalar(17),i.scalar(14),i.scalar(13)));
            wind(i,1,9,0,&mut push);
        }
        "SHD_Nat_Veg_Grass_AO"=>push(format!("layout B: B=t75.rgb*(1+{}*(COLOR0.a-1)); O=1; \
            specular occlusion o2.z=.2; translucency o2.w=0; core.pbr has no specular-occlusion field. \
            Rewritten Shaded vertex factors must not be substituted for original Game COLOR0",i.scalar(1))),
        "MAD_FakeForest"=>push("B=pow(saturate(interpolated COLOR0.rgb),2.2), R=1, M=0; \
            translucency o2.w=.5. Shaded computes power before interpolation; pow(lerp(c),2.2) differs from \
            lerp(pow(c,2.2)). Original Game input retention and a fragment colour operation are needed for equality".into()),
        "MAD_Screens"|"MAD_Screens_Simple"=>{
            let simple=template=="MAD_Screens_Simple";
            let (fps,delay,counts)=if simple {(i.scalar(4),0.0,i.vector(6))}else{(i.scalar(10),i.scalar(11),i.vector(13))};
            push(format!("flipbook UV: frame=FPS>=1 ? uint(cb0[0].x*FPS+delay) % (cols*rows) : 0; \
                FPS={fps}, video delay={delay}, stored SpriteCountXY={counts:?}; cols/rows=uint(max(SpriteCountXY,1)). \
                cell=((frame%cols+u)/cols,(rows-1-floor(frame/cols)+v)/rows). No core flipbook/UV animation field; \
                the reduced black branch is independent of the clock; other video states retain a named refusal"));
        }
        _=>{}
    }
    for &(key,binding) in &i.m.bindings {
        if let MaterialBinding::UvTransform{vec4}=binding
            && let Some(MaterialValue::Floats(f))=i.bound(binding)
            && let Some(motion)=uv_motion(f) {
            push(format!("property {key:08x}, cb3[{vec4}]: {motion}"));
        }
    }
    out
}

fn wind(i:&Inputs<'_>,first:u32,last:u32,debug:u32,push:&mut impl FnMut(String)) {
    let parameters=(first..=last).map(|r|(r,i.bound(MaterialBinding::Scalar{vec4:r})
        .or_else(||i.bound(MaterialBinding::Vector{vec4:r})))).collect::<Vec<_>>();
    let debug=if debug==0 {"this contract has no pixel wind-debug row".into()}
        else {format!("pixel wind-debug cb3[{debug}]={}; its noise-colour blend is omitted",i.scalar(debug))};
    push(format!("wind deformation is the retained vertex program, cb3 rows/values {parameters:?}; \
        no core static-material field encodes it; {debug}; phase/varying inputs remain the vertex program's named bindings"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{AnvilMaterial, MaterialFlags};

    fn material(template:&str,scalars:&[(u32,f32)]) -> AnvilMaterial {
        AnvilMaterial {index_id:0x123,template:None,template_name:Some(template.into()),
            bindings:scalars.iter().map(|&(r,_)|(r,MaterialBinding::Scalar{vec4:r})).collect(),
            values:scalars.iter().map(|&(r,v)|(r,MaterialValue::Float(v))).collect(),texture_set:Vec::new(),
            textures:Vec::new(),defaults:Vec::new(),frame:primase_core::frame::SourceFrame::WORLD,
            flags:MaterialFlags{blend_mode:0,alpha_display_mode:0,alpha_test_value:0,booleans:[0;35]},shaders:None}
    }

    #[test]
    fn vertex_notice_keeps_rgb_coverage_and_emission_separate() {
        let m=material("MAD_Tex1_Emissive",&[(2,4.0),(6,0.5),(7,1.0)]);
        let text=notices(&Inputs{m:&m}).join("\n");
        assert!(text.contains("1+1*(COLOR0.rgb-1)") && text.contains("coverage A=diffuse.a*0.5"));
        assert!(text.contains("E=B*(t77[0]*4/t23.x)") && text.contains("COLOR0.a is not an emission factor"));
        assert!(text.contains("Material 0x123"));
    }

    #[test]
    fn foliage_notice_preserves_zero_specular_occlusion_and_mask_lanes() {
        let m=material("MAD_Clutters_Grass",&[(10,0.75),(11,0.0),(16,0.5)]);
        let text=notices(&Inputs{m:&m}).join("\n");
        assert!(text.contains("specular occlusion o2.z=0") && text.contains("o2.w=t75.r*0.75"));
        assert!(text.contains("O=1+0.5*(t75.b-1)") && text.contains("coverage"));
        let m=material("MAD_Tex1_Leaves",&[(21,0.5),(25,0.25),(27,1.0),(31,0.75)]);
        let text=notices(&Inputs{m:&m}).join("\n");
        assert!(text.contains("sat(COLOR0.a+0.25)") && text.contains("o2.z=t78.b*0.5"));
        assert!(text.contains("o2.w=t78.r*0.75") && text.contains("A=t79.r"));
    }

    #[test]
    fn moving_uv_notice_retains_rates_without_guessing_the_engine_clock() {
        let text=uv_motion(&[2.0,3.0,0.0,0.0,0.25,-0.5,1.0,0.125]).unwrap();
        assert!(text.contains("TranslationSpeedU=0.25") && text.contains("TranslationSpeedV=-0.5"));
        assert!(text.contains("RotationSpeed=0.125") && text.contains("activation bytes"));
        assert!(uv_motion(&[1.0,1.0,0.0,0.0,0.0,0.0,0.0,0.0]).is_none());
    }
}
