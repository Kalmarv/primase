//! Surface-domain execution of a material's own pixel program. Unlike a repeating UV-square bake, this evaluates
//! the placement's real world positions, normals and vertex colours. Each triangle owns an atlas chart; neither
//! shared prototypes nor unrelated material variants acquire another placement's baked pixels.
//!
//! Input contracts: MAD_Cliffs_TriPlanar (material-spec §27.4 and verify_m17.py), and
//! MAD_Tex1_UniqueNormalGrad_Triplanar and MAD_OutBorder (their own ps_400000 disassemblies).
//! Texture/constant roles come from the template's registry bindings, not a second hand-written colour formula.
//! The shader performs its own mapping and normal blending. Unknown varying/constant/resource roles refuse
//! that primitive by name. G-buffer class is decoded from the program's output, not inferred from its template name.
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{PbrMaterial, TextureRef, Wrap};
use primase_core::shader::{NormalEncoding, SamplerState};
use primase_core::{ColorSpace, Component, Instance, Linear, MaterialVariants, Mesh, Name,
    NormalBasis, NormalConvention, NormalGreen, NormalX, NormalZ, Primitive, Semantic, Stream,
    TexFormat, TexRole, Texture, Topology, Transform, VertexColor, World};
use primase_image::sampling::{Levels, Transfer};
use primase_shader_exec::{Execution, Filtering, PixelShader, ProgramShader, SampledTexture, SamplerUse, View, Views};
use crate::components::{AnvilMaterial, AnvilRenderStatus, AnvilShaders, AnvilTextureMap};
use super::bake::{UnstatedAddress, material_rows};

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum SurfaceSediments { Omit, Refuse }
impl Choice for SurfaceSediments {
    const VALUES:&'static [(&'static str,Self)]=&[("omit",Self::Omit),("refuse",Self::Refuse)];
}

#[derive(Clone, Debug)]
pub struct BakeAnvilSurface {
    pub texels_per_metre: f32,
    pub max_size: u32,
    pub max_chart: u32,
    pub max_texels:u64,
    pub terrain_ao: f32,
    pub sediments: SurfaceSediments,
    pub mip_bias: f32,
    pub camera_distance:f32,
    pub address: UnstatedAddress,
}
pub const BAKE_ANVIL_SURFACE: PassSpec = PassSpec {
    name: "bake-anvil-surface",
    options: &[
        OptionSpec::required("texels-per-metre", "8", "surface chart density in world metres; refused rather than silently downsized"),
        OptionSpec::required("max-size", "2048", "maximum atlas side per primitive, power of two, 32 to 8192"),
        OptionSpec::required("max-chart", "256", "maximum triangle chart side including gutters, even, 8 to 2048"),
        OptionSpec::required("max-texels", "16777216", "total atlas texels per pass; four RGBA8 output maps, refused on capacity exhaustion"),
        OptionSpec::required("terrain-ao", "0.5", "stated terrain AO stand-in, 0 to 1"),
        OptionSpec::choice::<SurfaceSediments>("sediments", "omit terrain sediment blending explicitly, or refuse its unresolved engine views"),
        OptionSpec::required("mip-bias", "0", "engine cb1[41].y at the chart's own footprint"),
        OptionSpec::required("camera-distance", "1000000", "static camera above the primitive centre in source metres; states the distance-dependent detail blend"),
        OptionSpec::choice::<UnstatedAddress>("unstated-address", "explicit sampler assumption or refuse"),
    ],
    make: |g: &Given| {
        let number = |key: &str| g.required(key).parse::<f32>().map_err(|_| format!("{key}: expected number"));
        let out = BakeAnvilSurface { texels_per_metre: number("texels-per-metre")?,
            max_size: g.required("max-size").parse().map_err(|_| "max-size: expected integer")?,
            max_chart: g.required("max-chart").parse().map_err(|_| "max-chart: expected integer")?,
            max_texels:g.required("max-texels").parse().map_err(|_|"max-texels: expected integer")?,
            terrain_ao: number("terrain-ao")?, sediments:g.choice("sediments"), mip_bias: number("mip-bias")?,camera_distance:number("camera-distance")?, address: g.choice("unstated-address") };
        if !out.texels_per_metre.is_finite() || out.texels_per_metre <= 0.0
            || !(32..=8192).contains(&out.max_size) || !out.max_size.is_power_of_two()
            || !(8..=2048).contains(&out.max_chart) || !out.max_chart.is_multiple_of(2)
            || !out.terrain_ao.is_finite() || !(0.0..=1.0).contains(&out.terrain_ao)
            || !out.mip_bias.is_finite() || !out.camera_distance.is_finite() || out.camera_distance<0.0
            || out.max_texels==0 { return Err("invalid surface bake density/budget/engine choices".into()); }
        Ok(Box::new(out))
    },
};

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 { a.into_iter().zip(b).map(|(a,b)| a*b).sum() }
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1]*b[2]-a[2]*b[1], a[2]*b[0]-a[0]*b[2], a[0]*b[1]-a[1]*b[0]]
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { std::array::from_fn(|k| a[k]-b[k]) }
fn unit(a: [f32; 3]) -> Result<[f32; 3], String> {
    let l = dot(a,a).sqrt();
    if !l.is_finite() || l < 1e-8 { Err("surface has a zero/non-finite normal or edge".into()) }
    else { Ok(a.map(|v| v/l)) }
}
fn mix<const N: usize>(v: [[f32; N]; 3], b: [f32; 3]) -> [f32; N] {
    std::array::from_fn(|k| (0..3).map(|i| v[i][k]*b[i]).sum())
}
fn byte(v: f32) -> u8 { (v.clamp(0.0,1.0)*255.0).round_ties_even() as u8 }

/// Chart coordinates are affine barycentrics even for helper lanes outside the triangle. Keeping those helpers
/// on the same plane is essential for the pixel program's implicit LOD and derivative instructions.
fn barycentric(x: f32, y: f32, extent: f32) -> [f32; 3] {
    let u = (x-2.0)/extent;
    let v = (y-2.0)/extent;
    [1.0-u-v,u,v]
}
fn inside(b: [f32; 3]) -> bool { b.iter().all(|v| *v >= 0.0) }

struct Prepared {
    shader: ProgramShader,
    reading: super::gbuffer::Reading,
    buffers: Vec<(u32, Vec<[u32;4]>)>,
    views: Views,
}

/// These are input *meanings*, not material formulas. Their texture rows and arithmetic remain in the template's
/// own program. Keep contracts distinct: gradient masks use original UVs, OutBorder also needs its authored tangent
/// frame, and RawTerrain is entirely dependent on the engine's terrain caches (a named refusal until supplied).
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
enum Contract {Cliffs,UniqueGradient,OutBorder,RawTerrain}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
enum VaryingRole {Screen,Normal,WorldPosition,Clip,Colour,Uv,Tangent,NormalSign}
impl Contract {
    fn of(material:&AnvilMaterial)->Option<Self>{Self::named(material.template_name.as_deref()?)}
    fn named(template:&str)->Option<Self>{match template{
        "MAD_Cliffs_TriPlanar"=>Some(Self::Cliffs),
        "MAD_Tex1_UniqueNormalGrad_Triplanar"=>Some(Self::UniqueGradient),
        "MAD_OutBorder"=>Some(Self::OutBorder),
        "MAD_Raw_TerrainBlend"=>Some(Self::RawTerrain),
        _=>None,
    }}
    fn varying(self,name:&str,index:u32)->Option<VaryingRole>{
        if name=="SV_Position" && index==0{return Some(VaryingRole::Screen);}
        if self==Self::OutBorder{return match(name,index){
            ("TEXCOORD",0)=>Some(VaryingRole::Uv),
            ("TEXCOORD",1)=>Some(VaryingRole::WorldPosition),
            ("TEXCOORD",2)=>Some(VaryingRole::Tangent),
            ("TEXCOORD",3)=>Some(VaryingRole::NormalSign),
            ("TEXCOORD",4)=>Some(VaryingRole::Clip),_=>None,
        };}
        if self==Self::RawTerrain{return match(name,index){
            ("TEXCOORD",0)=>Some(VaryingRole::Normal),
            ("TEXCOORD",1)=>Some(VaryingRole::WorldPosition),
            ("TEXCOORD",5)=>Some(VaryingRole::Clip),_=>None,
        };}
        match(name,index){
        ("SV_Position",0)=>Some(VaryingRole::Screen),
        ("TEXCOORD",1)=>Some(VaryingRole::Normal),
        ("TEXCOORD",2)=>Some(VaryingRole::WorldPosition),
        ("TEXCOORD",6)=>Some(VaryingRole::Clip),
        ("COLOR",0) if self==Self::Cliffs=>Some(VaryingRole::Colour),
        ("TEXCOORD",0) if self==Self::UniqueGradient=>Some(VaryingRole::Uv),
        _=>None,
    }}

    fn terrain_requirements(self)->Option<&'static str>{
        (self==Self::RawTerrain).then_some("MAD_Raw_TerrainBlend surface inputs unresolved: t13 page addresses, t14 page levels, t15 terrain height atlas, t16 sediment IDs, t17 terrain colour atlas, t19 terrain AO, t20 sediment array, t22 sediment records; t45/t46 detail colour/normal caches, uint3D t50/t51 page tables, u5 feedback; cb0[47..50] terrain mapping/height/detail transition and cb0[146,148,149] virtual-cache layout; cb1[3..5].w camera position. AnvilTerrain's stored tiles are not these runtime GPU resources; no zero or unrelated texture substituted")
    }
}
impl BakeAnvilSurface {
    fn prepare(&self, world: &World, material: &AnvilMaterial,camera:[f32;3],side:u32) -> Result<Prepared, String> {
        let contract=Contract::of(material).ok_or("template has no established surface input contract")?;
        if let Some(reason)=contract.terrain_requirements(){return Err(reason.into());}
        if material.flags.blend_mode != 0 { return Err("blended surface: G-buffer alpha is not roughness".into()); }
        if material.flags.booleans.get(crate::components::MaterialFlags::ALPHA_TEST_ENABLED)==Some(&1) {
            return Err("alpha-tested surface requires a coverage-aware chart contract".into());
        }
        let programs = material.shaders.and_then(|e| world.get::<AnvilShaders>(e))
            .ok_or("template programs were not read")?;
        let key = super::resolve_shaders::gbuffer_key(&material.flags, programs)?;
        let bytes = programs.pixel(key).ok_or_else(|| format!("missing G-buffer pixel permutation {key:#x}"))?;
        let program = primase_dxbc::lift(bytes).map_err(|e| e.to_string())?;
        let reads = program.constant_reads()?;
        let shader = ProgramShader::new(program).map_err(|e| e.to_string())?;
        if self.sediments==SurfaceSediments::Refuse && shader.textures().iter().any(|slot|matches!(*slot,13|14|16|20|22)) {
            return Err("terrain sediment engine views t13/t14/t16/t20/t22 unresolved; caller chose sediments=refuse".into());
        }
        for input in shader.interface().inputs.iter().filter(|v| v.used) {
            if contract.varying(&input.label.name,input.label.index).is_none() {
                return Err(format!("unknown surface varying {}{}", input.label.name,input.label.index));
            }
            if input.provenance.d3d.is_none() { return Err("surface varying has no register provenance".into()); }
        }
        let rows = material_rows(material)?;
        let mut buffers = Vec::new();
        for &(slot, count) in shader.constant_buffers() {
            let mut buffer = vec![[0u32;4]; count as usize];
            let id = reads.buffer_at(0,slot).ok_or_else(|| format!("cb{slot}: no constant read provenance"))?;
            if reads.indexed(id) { return Err(format!("cb{slot}: dynamic engine/material indexing has no established surface binding")); }
            for word in reads.words(id) {
                let (row,lane) = (word/4,word%4);
                let value = match (slot,row,lane) {
                    (3,row,_) => *rows.get(&row).ok_or_else(|| format!("cb3[{row}]: template binds no property"))?
                        .get(lane as usize).ok_or("material lane")?,
                    // Normal already transformed into world/source axes: the three normal rows are identity.
                    (2,0..=2,0..=2) => if row==lane { 1.0 } else { 0.0 },
                    (2,0..=2,3) => 0.0,
                    (1,41,1) => self.mip_bias,
                    (1,0,0..=2) if contract==Contract::UniqueGradient=>camera[lane as usize],
                    (1,37,0|1) => 1.0 / side as f32,
                    (1,37,2|3) => side as f32,
                    // Virtual-terrain coordinates are irrelevant with the explicit uniform engine textures below.
                    (0,47,0) | (0,48,0|1) if matches!(contract,Contract::Cliffs|Contract::OutBorder) => 1.0,
                    (0,47,1..=3) | (0,48,2|3) if matches!(contract,Contract::Cliffs|Contract::OutBorder) => 0.0,
                    _ => return Err(format!("unknown engine constant cb{slot}[{row}].{}", ["x","y","z","w"][lane as usize])),
                };
                let target = buffer.get_mut(row as usize).ok_or("constant row outside declaration")?;
                target[lane as usize] = value.to_bits();
            }
            buffers.push((slot,buffer));
        }
        let mut views = BTreeMap::new();
        for &slot in shader.textures() {
            let view = match slot {
                13|14|16|20|22 if matches!(contract,Contract::Cliffs|Contract::OutBorder) => View::Constant([0.0;4]), // explicit no-sediment preview
                19 if contract==Contract::Cliffs => View::Constant([self.terrain_ao;4]),
                75.. => {
                    let binding = material.textures.iter().find(|v| v.shader_slot == slot-75)
                        .ok_or_else(|| format!("t{slot}: template texture role is unknown"))?;
                    let entity = binding.texture.ok_or_else(|| format!("t{slot}, property {:08x}: unresolved texture",binding.key))?;
                    let texture = world.get::<Texture>(entity).ok_or_else(|| format!("t{slot}: resolved resource has no texture payload"))?;
                    let srgb = world.get::<AnvilTextureMap>(entity).map_or(texture.color_space==ColorSpace::Srgb, |t| t.field_25==1);
                    View::Image(Arc::new(SampledTexture::new(Levels::of(texture).map_err(|e| e.to_string())?,
                        if srgb {Transfer::SrgbColour} else {Transfer::Stored},
                        SamplerUse::of(&SamplerState::default(), self.address.unstated())?, Filtering::Trilinear)?))
                },
                _ => return Err(format!("unknown engine resource t{slot}")),
            };
            views.insert(slot,view);
        }
        let reading = super::gbuffer::Reading::new(shader.interface(), 1)?;
        Ok(Prepared {shader,reading,buffers,views:Views::refusing(views)})
    }
}

struct Images { base: Vec<u8>, normal: Vec<u8>, orm: Vec<u8>, ao: Vec<u8> }
impl Images {
    fn new(size: usize) -> Self { Self {base:vec![0;size*size*4],normal:vec![0;size*size*4],
        orm:vec![0;size*size*4],ao:vec![0;size*size*4]} }
    fn put(&mut self, at: usize, output: &Execution, reading: &super::gbuffer::Reading, tbn: [[f32;3];3], frame: primase_core::frame::SourceFrame) -> Result<(),String> {
        if output.discarded { return Err("surface permutation discards pixels; opaque chart contract does not cover it".into()); }
        let mut o = [[0.0;4];4];
        for (target,dst) in o.iter_mut().enumerate() {
            if output.output_masks.get(target).copied()!=Some(15) { return Err(format!("G-buffer o{target} is not fully written")); }
            *dst = output.outputs[target].map(f32::from_bits);
            if dst.iter().any(|v| !v.is_finite()) {return Err(format!("non-finite G-buffer o{target}"));}
        }
        let (_,class)=NormalEncoding::decode_octahedral_diamond([o[1][0],o[1][1],o[1][2]]);
        if class!=1 {return Err(format!("surface program writes shading class {class}; established TriPlanar contract is class 1"));}
        let texel = reading.read(|target| output.outputs.get(target).zip(output.output_masks.get(target))
            .map(|(v, mask)| (v.map(f32::from_bits), *mask)))?;
        if texel.specular_occlusion!=1.0 || texel.translucency!=0.0 || texel.emission!=[0.0;3] {
            return Err("surface chart cannot represent non-neutral specular occlusion/translucency or nonzero emission; original material retained".into());
        }
        if texel.base.iter().any(|v| !(0.0..=1.0).contains(v)) {
            return Err("surface chart albedo is outside 0 to 1; RGBA8 PBR would clip the program's output".into());
        }
        let n = frame.direction(texel.normal);
        let normal = tbn.map(|axis| dot(n,axis));
        self.base[at..at+4].copy_from_slice(&[
            byte(primase_image::linear_to_srgb(f64::from(texel.base[0])) as f32),
            byte(primase_image::linear_to_srgb(f64::from(texel.base[1])) as f32),
            byte(primase_image::linear_to_srgb(f64::from(texel.base[2])) as f32),255]);
        self.normal[at..at+4].copy_from_slice(&[byte(normal[0]*0.5+0.5),byte(normal[1]*0.5+0.5),byte(normal[2]*0.5+0.5),255]);
        self.orm[at..at+4].copy_from_slice(&[255,byte(texel.roughness),byte(texel.metallic),255]);
        let ao=byte(texel.occlusion); self.ao[at..at+4].copy_from_slice(&[ao,ao,ao,255]);
        Ok(())
    }
    fn copy(&mut self, from: usize, to: usize) {
        for pixels in [&mut self.base,&mut self.normal,&mut self.orm,&mut self.ao] {
            let p:[u8;4]=pixels[from..from+4].try_into().expect("RGBA pixel");
            pixels[to..to+4].copy_from_slice(&p);
        }
    }
}

impl BakeAnvilSurface {
    fn bake_primitive(&self, world: &World, primitive: &Primitive, material: &AnvilMaterial,
        transform: Transform,remaining_texels:u64) -> Result<(Primitive,Images,u32),String> {
        let contract=Contract::of(material).ok_or("template has no surface input contract")?;
        if let Some(reason)=contract.terrain_requirements(){return Err(reason.into());}
        if primitive.topology!=Topology::Triangles {return Err("surface is not a triangle list".into());}
        let permitted=BTreeSet::from([Semantic::Position,Semantic::Normal,Semantic::Tangent,Semantic::TexCoord(0),Semantic::Color(0)]);
        if primitive.streams.keys().any(|s| !permitted.contains(s)) {return Err("surface has extra UV/skin/blend streams whose chart remap is not established".into());}
        let p=primitive.positions().ok_or("surface has no positions")?;
        let normals=match primitive.streams.get(&Semantic::Normal) {Some(Stream::F32x3(n)) if n.len()==p.len()=>n,_=>return Err("surface has no matching normal stream".into())};
        let source_tangents=if contract==Contract::OutBorder {
            match primitive.streams.get(&Semantic::Tangent){
                Some(Stream::F32x4(t)) if t.len()==p.len() && t.iter().all(|v|v.iter().all(|x|x.is_finite()) && v[3].abs()==1.0)=>Some(t),
                _=>return Err("MAD_OutBorder requires the original tangent and bitangent sign; no chart tangent is substituted for its source varying".into()),
            }
        }else{None};
        let colours=match primitive.streams.get(&Semantic::Color(0)) {
            Some(Stream::F32x4(c)) if c.len()==p.len()=>Some(c),
            None if contract!=Contract::Cliffs=>None,
            _=>return Err("Cliffs requires matching COLOR0; no constant colour is invented".into()),
        };
        let source_uv=match primitive.streams.get(&Semantic::TexCoord(0)) {
            Some(Stream::F32x2(uv)) if uv.len()==p.len()=>Some(uv),
            None if !matches!(contract,Contract::UniqueGradient|Contract::OutBorder)=>None,
            _=>return Err("surface requires matching original TEXCOORD0 for its template textures".into()),
        };
        let ids=primitive.indices.clone().unwrap_or_else(||(0..p.len() as u32).collect());
        if ids.is_empty(){return Err("surface has no triangle indices".into());}
        if !ids.len().is_multiple_of(3) || ids.iter().any(|i| *i as usize>=p.len()) {return Err("surface indices outside triangle positions".into());}
        let linear=Linear::new([transform.cols[0],transform.cols[1],transform.cols[2]]);
        if linear.similarity().is_none() || linear.mirrors() {return Err("TriPlanar placement is mirrored/sheared/nonuniform; normal-frame contract not established".into());}
        let mut charts=Vec::new();
        let mut side=32;
        // Shelf pack the actual requested densities. Capacity failures retain the original primitive/material.
        for triangle in ids.as_chunks::<3>().0 {
            let v:[usize;3]=[triangle[0] as usize,triangle[1] as usize,triangle[2] as usize];
            let positions=v.map(|i|transform.apply(p[i]));
            let edge=(0..3).map(|i| dot(sub(positions[(i+1)%3],positions[i]),sub(positions[(i+1)%3],positions[i])).sqrt()).fold(0.0f32,f32::max);
            let chart=((edge*self.texels_per_metre).ceil() as u32+5).max(8).next_multiple_of(2);
            if chart>self.max_chart {return Err(format!("surface chart needs {chart}px at {} texels/m, exceeds max-chart {}",self.texels_per_metre,self.max_chart));}
            charts.push((v,positions,chart)); side=side.max(chart.next_power_of_two());
        }
        let placements=loop {
            if side>self.max_size {return Err(format!("surface atlas exceeds {}px budget at {} texels/m",self.max_size,self.texels_per_metre));}
            let (mut x,mut y,mut row)=(0,0,0);
            let mut packed=Vec::new();
            for (_,_,chart) in &charts {
                if x+chart>side {x=0;y+=row;row=0;}
                if y+chart>side {break;}
                packed.push((x,y)); x+=chart;row=row.max(*chart);
            }
            if packed.len()==charts.len() {break packed;} side*=2;
        };
        let needed=u64::from(side)*u64::from(side);
        if needed>remaining_texels {return Err(format!("surface capacity: atlas needs {needed} texels, {remaining_texels} remain in caller max-texels budget"));}
        // Capacity is checked before decoding input images or running any pixel program.
        let mut centre=[0.0f64;3];
        for point in p {
            let point=transform.apply(*point);
            if point.iter().any(|v|!v.is_finite()){return Err("surface world position is not finite".into());}
            for k in 0..3 {centre[k]+=f64::from(point[k])/p.len() as f64;}
        }
        let mut camera=material.frame.point_to_source(centre.map(|v|v as f32));camera[2]+=self.camera_distance;
        if camera.iter().any(|v|!v.is_finite()){return Err("surface virtual camera is not finite".into());}
        let prepared=self.prepare(world,material,camera,side)?;
        let buffers:Vec<_>=prepared.buffers.iter().map(|(s,b)|(*s,b.as_slice())).collect();
        let mut images=Images::new(side as usize);
        let (mut positions,mut output_normals,mut tangents,mut uvs,mut output_colours,mut original_uvs)=
            (Vec::new(),Vec::new(),Vec::new(),Vec::new(),Vec::new(),Vec::new());
        for ((vertices,points,chart),(cx,cy)) in charts.into_iter().zip(placements) {
            let nn=vertices.map(|i|linear.normal(normals[i]).ok_or("singular normal transform"))
                .into_iter().collect::<Result<Vec<_>,_>>()?;
            let colour=colours.map(|c|vertices.map(|i|c[i]));
            let mesh_uv=source_uv.map(|uv|vertices.map(|i|uv[i]));
            if let Some(ts)=source_tangents
                && vertices.iter().any(|i|ts[*i][3]!=ts[vertices[0]][3]) {
                    return Err("surface triangle crosses authored tangent handedness; a split frame is required".into());
            }
            let authored_tangents=source_tangents.map(|ts|vertices.map(|i|{
                let t=ts[i];let direction=sub(transform.apply([t[0],t[1],t[2]]),transform.apply([0.0;3]));
                [direction[0],direction[1],direction[2],t[3]]
            }));
            let tangent=unit(sub(points[1],points[0]))?;
            let extent=(chart-5) as f32;
            let mut valid=vec![false;(chart*chart) as usize];
            for y in (0..chart).step_by(2) {for x in (0..chart).step_by(2) {
                let bc=std::array::from_fn::<_,4,_>(|lane|barycentric((x+lane as u32%2) as f32+0.5,(y+lane as u32/2) as f32+0.5,extent));
                if bc.iter().all(|b|!inside(*b)) {continue;}
                let mut inputs:[Vec<[u32;4]>;4]=std::array::from_fn(|_|vec![[0;4];prepared.shader.input_count()]);
                let mut bases=[[[0.0;3];3];4];
                for lane in 0..4 {
                    let pos=mix(points,bc[lane]); let n=unit(mix([nn[0],nn[1],nn[2]],bc[lane]))?;
                    let authored=authored_tangents.map(|ts|mix(ts,bc[lane]));
                    let direction=authored.map_or(tangent,|t|[t[0],t[1],t[2]]);
                    let sign=authored.map_or(1.0,|t|t[3].signum());
                    if sign==0.0{return Err("source tangent sign crosses zero inside a surface triangle".into());}
                    let t=unit(sub(direction,n.map(|v|v*dot(direction,n))))?; let b=cross(n,t).map(|v|v*sign);
                    bases[lane]=[t,b,n];
                    let source_pos=material.frame.point_to_source(pos);
                    let source_n=material.frame.direction_to_source(n);
                    let source_sign=if material.frame.flips_winding(){-sign}else{sign};
                    for varying in prepared.shader.interface().inputs.iter().filter(|v|v.used) {
                        let role=contract.varying(&varying.label.name,varying.label.index).ok_or("unvalidated varying")?;
                        let value=match role {
                            VaryingRole::Screen=>[(cx+x+lane as u32%2) as f32+0.5,(cy+y+lane as u32/2) as f32+0.5,0.0,1.0],
                            VaryingRole::Colour=>mix(colour.ok_or("source COLOR0 missing")?,bc[lane]),
                            VaryingRole::Normal=>[source_n[0],source_n[1],source_n[2],1.0],
                            VaryingRole::NormalSign=>[source_n[0],source_n[1],source_n[2],source_sign],
                            VaryingRole::Tangent=>{let t=material.frame.direction_to_source(t);[t[0],t[1],t[2],0.0]},
                            VaryingRole::WorldPosition=>[source_pos[0],source_pos[1],source_pos[2],0.0],
                            VaryingRole::Clip=>[0.0,0.0,0.0,1.0],
                            VaryingRole::Uv=>{let uv=mix(mesh_uv.ok_or("source TEXCOORD0 missing")?,bc[lane]);[uv[0],uv[1],0.0,0.0]},
                        };
                        let reg=varying.provenance.d3d.ok_or("missing input register")?.register as usize;
                        *inputs[lane].get_mut(reg).ok_or("input register beyond shader input count")?=value.map(f32::to_bits);
                    }
                }
                let output=prepared.shader.execute_quad(inputs.each_ref().map(|v|v.as_slice()),&buffers,&mut &prepared.views).map_err(|e|e.to_string())?;
                for lane in 0..4 {if inside(bc[lane]) {
                    let(px,py)=(x+lane as u32%2,y+lane as u32/2);
                    images.put((((cy+py)*side+cx+px)*4) as usize,&output[lane],&prepared.reading,bases[lane],material.frame)?;
                    valid[(py*chart+px) as usize]=true;
                }}
            }}
            // Dilate the gutter and unused chart half from the closest point in this chart, never another face.
            let mut queue:VecDeque<_>=valid.iter().enumerate().filter_map(|(i,v)|v.then_some(i)).collect();
            if queue.is_empty(){return Err("surface triangle has no raster coverage".into());}
            while let Some(from)=queue.pop_front() {
                let (x,y)=(from as u32%chart,from as u32/chart);
                for (nx,ny) in [(x.wrapping_sub(1),y),(x+1,y),(x,y.wrapping_sub(1)),(x,y+1)] {
                    if nx>=chart || ny>=chart {continue;}
                    let to=(ny*chart+nx) as usize;
                    if valid[to] {continue;}
                    images.copy((((cy+y)*side+cx+x)*4) as usize,(((cy+ny)*side+cx+nx)*4) as usize);
                    valid[to]=true;queue.push_back(to);
                }
            }
            for k in 0..3 {
                // Keep geometry local: normals are converted back so the same authored instance transform applies.
                positions.push(p[vertices[k]]);output_normals.push(normals[vertices[k]]);
                if let Some(c)=colours {output_colours.push(c[vertices[k]]);}
                if let Some(uv)=source_uv {original_uvs.push(uv[vertices[k]]);}
                if let Some(ts)=source_tangents {
                    // Keep the authored frame for both the retained Game vertex program and the baked normal.
                    tangents.push(ts[vertices[k]]);
                } else {
                    let local_t=unit(sub(p[vertices[1]],p[vertices[0]]))?;
                    let n=unit(normals[vertices[k]])?;let t=unit(sub(local_t,n.map(|v|v*dot(local_t,n))))?;
                    tangents.push([t[0],t[1],t[2],1.0]);
                }
                let a=[[2.0,2.0],[2.0+extent,2.0],[2.0,2.0+extent]][k];
                uvs.push([(cx as f32+a[0])/side as f32,(cy as f32+a[1])/side as f32]);
            }
        }
        let mut streams=BTreeMap::from([
            (Semantic::Position,Stream::F32x3(positions)),(Semantic::Normal,Stream::F32x3(output_normals)),
            (Semantic::Tangent,Stream::F32x4(tangents)),(Semantic::TexCoord(1),Stream::F32x2(uvs))]);
        if source_uv.is_some(){streams.insert(Semantic::TexCoord(0),Stream::F32x2(original_uvs));}
        if colours.is_some(){streams.insert(Semantic::Color(0),Stream::F32x4(output_colours));}
        Ok((Primitive{topology:Topology::Triangles,streams,indices:None,material:None},images,side))
    }
}

impl Pass for BakeAnvilSurface {
    fn name(&self)-> &'static str {"bake-anvil-surface"}
    fn options(&self)->Options {Options::from([("texels-per-metre".into(),self.texels_per_metre.to_string()),
        ("max-size".into(),self.max_size.to_string()),("max-chart".into(),self.max_chart.to_string()),
        ("max-texels".into(),self.max_texels.to_string()),
        ("terrain-ao".into(),self.terrain_ao.to_string()),("mip-bias".into(),self.mip_bias.to_string()),
        ("sediments".into(),self.sediments.text().into()),
        ("camera-distance".into(),self.camera_distance.to_string()),
        ("unstated-address".into(),self.address.text().into())])}
    fn may_lack_inputs(&self)->bool {true}
    fn requires(&self)->&[&'static str] {&[Mesh::NAME,AnvilMaterial::NAME,AnvilShaders::NAME]}
    fn provides(&self)->&[&'static str] {&[Mesh::NAME,Texture::NAME,PbrMaterial::NAME,VertexColor::NAME,AnvilRenderStatus::NAME,
        crate::components::AnvilSurfaceBake::NAME,Name::NAME,primase_core::shader::ShaderBinding::NAME]}
    fn modifies(&self)->&[&'static str] {&[Mesh::NAME,Instance::NAME,AnvilRenderStatus::NAME]}
    fn approximates(&self)-> &'static str {"surface atlas at caller density/budget; stated static camera for detail blending; uniform terrain AO, caller-stated sediment omission or refusal, stated sampler addressing; static draw without motion"}
    fn run(&self,world:&mut World,log:&mut PassLog)->Result<(),String> {
        let instances:Vec<_>=world.iter::<Instance>().map(|(e,i)|(e,i.prototype)).collect();
        let mut texels=0u64;
        for (entity,prototype) in instances {
            let Some(mesh)=world.get::<Mesh>(prototype).cloned() else {continue;};
            if !mesh.primitives().iter().any(|p|p.material.and_then(|m|world.get::<AnvilMaterial>(m))
                .is_some_and(|m|Contract::of(m).is_some())) {continue;}
            let Some(transform)=world.get::<Transform>(entity).copied() else {continue;};
            if world.has::<MaterialVariants>(prototype) {
                log.omit("TriPlanar surface bake: material variants require separate chart sets",1);continue;
            }
            let mut primitives=mesh.primitives().to_vec();let mut changed=false;
            for p in &mut primitives {
                let Some(material_id)=p.material else {continue;};
                let Some(material)=world.get::<AnvilMaterial>(material_id).cloned() else {continue;};
                if Contract::of(&material).is_none(){continue;}
                let template=material.template_name.as_deref().unwrap_or("unknown");
                match self.bake_primitive(world,p,&material,transform,self.max_texels.saturating_sub(texels)) {
                    Err(reason)=>{
                        let reason=format!("{template} surface bake: {reason}");
                        world.insert(material_id,AnvilRenderStatus{source:format!("Material {:#x}",material.index_id),
                            template:material.template_name.clone(),reason:reason.clone(),unsupported:true});log.omit(&reason,1);
                    },
                    Ok((mut primitive,images,size))=>{
                        let needed=u64::from(size)*u64::from(size);
                        if texels.saturating_add(needed)>self.max_texels {
                            let reason=format!("{template} surface capacity: {texels}+{needed} texels exceeds caller max-texels {}",self.max_texels);
                            world.insert(material_id,AnvilRenderStatus{source:format!("Material {:#x}",material.index_id),
                                template:material.template_name.clone(),reason:reason.clone(),unsupported:true});
                            log.omit(&reason,1);continue;
                        }
                        texels+=needed;
                        let spawn=|world:&mut World,pixels,space,role,normal| {
                            let e=world.spawn();world.insert(e,Texture{width:size,height:size,format:TexFormat::Rgba8Unorm,
                                levels:vec![pixels],color_space:space,role,normal});e
                        };
                        let normal=NormalConvention{green:NormalGreen::Up,z:NormalZ::Stored,x:NormalX::Red,basis:NormalBasis::TangentBitangent};
                        if images.ao.as_chunks::<4>().0.iter().any(|v|v[0]!=255) {
                            log.approximate(&format!("{template}: {}", super::gbuffer::OCCLUSION_APPROXIMATION),1);
                        }
                        let base=spawn(world,images.base,ColorSpace::Srgb,TexRole::Color,None);
                        let normals=spawn(world,images.normal,ColorSpace::Linear,TexRole::Normal,Some(normal));
                        let orm=spawn(world,images.orm,ColorSpace::Linear,TexRole::Mask,None);
                        let ao=spawn(world,images.ao,ColorSpace::Linear,TexRole::Mask,None);
                        let sampled=|e|TextureRef{texcoord:1,wrap:[Wrap::Clamp;2],..TextureRef::new(e)};
                        let m=world.spawn();world.insert(m,PbrMaterial{base_color_texture:Some(sampled(base)),
                            normal_texture:Some(sampled(normals)),metallic:1.0,roughness:1.0,
                            metallic_roughness_texture:Some(sampled(orm)),occlusion_texture:Some(sampled(ao)),..Default::default()});
                        world.insert(m,VertexColor::Ignored);
                        world.insert(m,crate::components::AnvilSurfaceBake{material:material_id,atlas_size:size,
                            texels_per_metre:self.texels_per_metre});
                        if let Some(binding)=world.get::<primase_core::shader::ShaderBinding>(material_id).cloned() {
                            world.insert(m,binding);
                        }
                        world.insert(m,Name(format!("Material {:#x}: placement {} surface bake",material.index_id,entity.index())));
                        let terrain_notice=if self.sediments==SurfaceSediments::Omit && matches!(Contract::of(&material),Some(Contract::Cliffs|Contract::OutBorder)){
                            "; preview omits terrain sediments via the stated zero sediment-ID view"
                        }else{""};
                        world.insert(m,AnvilRenderStatus{source:"bake-anvil-surface".into(),template:material.template_name.clone(),
                            reason:format!("bake-anvil-surface: {template} own G-buffer program, placement world position/normal and original mesh inputs; {size}px TEXCOORD1 atlas at {} texels/m; static camera {} m above primitive centre{terrain_notice}",self.texels_per_metre,self.camera_distance),unsupported:false});
                        primitive.material=Some(m);*p=primitive;changed=true;
                        log.count("placement primitives surface-baked with their own program",1);
                    }
                }
            }
            if changed {
                let copy=world.duplicate(prototype).ok_or("surface prototype vanished")?;
                world.insert(copy,Mesh::new(primitives));
                world.get_mut::<Instance>(entity).ok_or("surface placement vanished")?.prototype=copy;
            }
        }
        Ok(())
    }
}

#[cfg(test)] mod tests {
    use super::*;
    #[test]fn surface_templates_keep_distinct_mesh_input_contracts(){
        assert_eq!(Contract::named("MAD_Tex1_UniqueNormalGrad_Triplanar"),Some(Contract::UniqueGradient));
        assert_eq!(Contract::UniqueGradient.varying("TEXCOORD",0),Some(VaryingRole::Uv));
        assert_eq!(Contract::UniqueGradient.varying("COLOR",0),None);
        assert_eq!(Contract::Cliffs.varying("COLOR",0),Some(VaryingRole::Colour));
        assert_eq!(Contract::Cliffs.varying("TEXCOORD",0),None);
        assert_eq!(Contract::UniqueGradient.varying("TEXCOORD",7),None);
        assert_eq!(Contract::named("MAD_RawTerrainBlend"),None);
        assert_eq!(Contract::named("MAD_Raw_TerrainBlend"),Some(Contract::RawTerrain));
        assert_eq!(Contract::named("MAD_OutBorder"),Some(Contract::OutBorder));
        assert_eq!(Contract::OutBorder.varying("TEXCOORD",2),Some(VaryingRole::Tangent));
        assert_eq!(Contract::OutBorder.varying("TEXCOORD",3),Some(VaryingRole::NormalSign));
        assert_eq!(Contract::RawTerrain.varying("TEXCOORD",1),Some(VaryingRole::WorldPosition));
        let missing=Contract::RawTerrain.terrain_requirements().unwrap();
        assert!(missing.contains("t50/t51") && missing.contains("u5 feedback") && missing.contains("cb0[146,148,149]"));
    }
    #[test]fn gradient_requires_mask_uv_and_checks_capacity_before_shader_inputs(){
        let pass=BakeAnvilSurface{texels_per_metre:8.0,max_size:2048,max_chart:256,max_texels:1024,
            terrain_ao:0.5,sediments:SurfaceSediments::Omit,mip_bias:0.0,camera_distance:1000000.0,address:UnstatedAddress::Wrap};
        let material=AnvilMaterial{index_id:17,template:None,template_name:Some("MAD_Tex1_UniqueNormalGrad_Triplanar".into()),
            bindings:vec![],values:vec![],texture_set:vec![],textures:vec![],defaults:vec![],
            frame:primase_core::frame::SourceFrame::WORLD,shaders:None,
            flags:crate::components::MaterialFlags{blend_mode:0,alpha_display_mode:0,alpha_test_value:0,booleans:[0;35]}};
        let mut primitive=Primitive{topology:Topology::Triangles,indices:None,material:None,streams:BTreeMap::from([
            (Semantic::Position,Stream::F32x3(vec![[0.0,0.0,0.0],[1.0,0.0,0.0],[0.0,1.0,0.0]])),
            (Semantic::Normal,Stream::F32x3(vec![[0.0,0.0,1.0];3]))])};
        let world=World::new();
        let failure=|p:&Primitive|match pass.bake_primitive(&world,p,&material,Transform::IDENTITY,0){
            Err(reason)=>reason,Ok(_)=>panic!("no budget or shader programs were supplied")};
        assert!(failure(&primitive).contains("original TEXCOORD0"));
        primitive.streams.insert(Semantic::TexCoord(0),Stream::F32x2(vec![[0.0,0.0],[0.5,0.0],[0.0,0.5]]));
        assert!(failure(&primitive).contains("surface capacity"));
        assert!(!failure(&primitive).contains("programs were not read"));
    }
    #[test]fn outborder_refuses_a_missing_authored_frame_and_raw_terrain_names_its_engine_resources(){
        let pass=BakeAnvilSurface{texels_per_metre:8.0,max_size:2048,max_chart:256,max_texels:1024,
            terrain_ao:0.5,sediments:SurfaceSediments::Omit,mip_bias:0.0,camera_distance:1000000.0,address:UnstatedAddress::Wrap};
        let mut material=AnvilMaterial{index_id:18,template:None,template_name:Some("MAD_OutBorder".into()),
            bindings:vec![],values:vec![],texture_set:vec![],textures:vec![],defaults:vec![],
            frame:primase_core::frame::SourceFrame::WORLD,shaders:None,
            flags:crate::components::MaterialFlags{blend_mode:0,alpha_display_mode:0,alpha_test_value:0,booleans:[0;35]}};
        let primitive=Primitive{topology:Topology::Triangles,indices:None,material:None,streams:BTreeMap::from([
            (Semantic::Position,Stream::F32x3(vec![[0.0,0.0,0.0],[1.0,0.0,0.0],[0.0,1.0,0.0]])),
            (Semantic::Normal,Stream::F32x3(vec![[0.0,0.0,1.0];3])),
            (Semantic::TexCoord(0),Stream::F32x2(vec![[0.0,0.0];3]))])};
        let world=World::new();
        let reason=match pass.bake_primitive(&world,&primitive,&material,Transform::IDENTITY,0){
            Err(reason)=>reason,Ok(_)=>panic!("OutBorder cannot invent a tangent")};
        assert!(reason.contains("original tangent and bitangent sign"));
        material.template_name=Some("MAD_Raw_TerrainBlend".into());
        let reason=match pass.bake_primitive(&world,&primitive,&material,Transform::IDENTITY,0){
            Err(reason)=>reason,Ok(_)=>panic!("terrain shader cannot invent engine caches")};
        assert!(reason.contains("t15 terrain height atlas") && reason.contains("t45/t46") && reason.contains("u5 feedback"));
    }
    #[test] fn chart_helper_lanes_stay_affine_beyond_the_triangle() {
        let a=barycentric(2.5,2.5,16.0);let b=barycentric(3.5,2.5,16.0);
        assert_eq!(b[1]-a[1],1.0/16.0);
        assert!(!inside(barycentric(22.5,22.5,16.0)));
        assert_eq!(barycentric(22.5,22.5,16.0).iter().sum::<f32>(),1.0);
    }
    #[test] fn a_uniform_scale_is_accepted_but_a_shear_is_not() {
        assert!(Linear::new([[2.0,0.0,0.0],[0.0,2.0,0.0],[0.0,0.0,2.0]]).similarity().is_some());
        assert!(Linear::new([[1.0,0.0,0.0],[1.0,1.0,0.0],[0.0,0.0,1.0]]).similarity().is_none());
    }
    #[test]fn layout_is_read_from_the_programs_encoded_class_and_world_normal_is_rebased(){
        let mut images=Images::new(1);
        let fixture = primase_dxbc::lift(include_bytes!("../../../../toolkit/dxbc/tests/fixtures/modes.dxbc")).unwrap();
        let reading = super::super::gbuffer::Reading::new(&fixture.interface, 1).unwrap();
        let mut output=Execution{outputs:vec![
            [0.25f32,0.5,0.75,0.6].map(f32::to_bits),
            [1.0f32,1.0,9.0/255.0,0.3].map(f32::to_bits),
            [0.0f32;4].map(f32::to_bits),[1.0f32,0.0,1.0,0.0].map(f32::to_bits)],
            output_masks:vec![15;4],..Default::default()};
        images.put(0,&output,&reading,[[1.0,0.0,0.0],[0.0,1.0,0.0],[0.0,0.0,1.0]],
            primase_core::frame::SourceFrame::WORLD).unwrap();
        assert_eq!(images.normal,[128,128,255,255]);assert_eq!(images.orm,[255,153,76,255]);
        output.outputs[1][2]=(41.0f32/255.0).to_bits();
        assert!(images.put(0,&output,&reading,[[1.0,0.0,0.0],[0.0,1.0,0.0],[0.0,0.0,1.0]],
            primase_core::frame::SourceFrame::WORLD).unwrap_err().contains("class 5"));
    }
}
