//! Coarse ground is TNL terrain, never FarFake object proxies. Height scaling is farview.md §6.2's
//! `raw * range / 2^20 * 2^(8-level)`. A zipper joins differently sampled concentric rectangle edges.
//! The inner edge uses every near-terrain sample and exactly its height; a stated band morphs to the coarse data.
//! Colour triangles are clipped at node boundaries, preserving each node's stored BC1 tile and border texels.
use std::collections::BTreeMap;
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{PbrMaterial, TextureRef, Wrap};
use primase_core::{ColorSpace, Component, Mesh, Name, Primitive, Semantic, Stream, TexFormat,
    TexRole, Texture, Topology, Transform, World};
use crate::components::{AnvilFarTerrain, AnvilRenderStatus, AnvilTerrain, TerrainTile};

#[derive(Clone, Debug)]
pub struct LowerAnvilFarTerrain { pub range: f64, pub offset: f64, pub seam: f64 }
pub const LOWER_ANVIL_FAR_TERRAIN: PassSpec = PassSpec {
    name: "lower-anvil-far-terrain",
    options: &[
        OptionSpec::required("range", "5000", "same height range as near terrain"),
        OptionSpec::required("offset", "0", "same vertical offset as near terrain"),
        OptionSpec::required("seam", "16", "metres over which the near border height morphs to coarse terrain"),
    ], make: |g: &Given| {
        let number=|k:&str|g.required(k).parse::<f64>().map_err(|_|format!("{k}: expected number"));
        let out=LowerAnvilFarTerrain{range:number("range")?,offset:number("offset")?,seam:number("seam")?};
        if !out.range.is_finite() || out.range<=0.0 || !out.offset.is_finite() || !out.seam.is_finite()
            || out.seam<=0.0 {return Err("invalid far terrain height/seam choices".into());}
        Ok(Box::new(out))
    },
};

fn spacing(t: &AnvilTerrain) -> f64 {t.side / f64::from(1u32<<t.level) / 128.0}
fn sample(t:&AnvilTerrain,p:[f64;2],range:f64,offset:f64)->Result<f64,String> {
    if t.level>8 || !t.side.is_finite() || t.side<=0.0 {return Err("far terrain: invalid quadtree".into());}
    let node=t.side/f64::from(1u32<<t.level);
    let c=((p[0]-t.origin[0])/node).floor() as i64;
    let r=((p[1]-t.origin[1])/node).floor() as i64;
    // A boundary can be sampled from the preceding tile's two-texel border when the next tile is not loaded.
    let tile=t.tiles.iter().find(|v|i64::from(v.column)==c && i64::from(v.row)==r)
        .or_else(||t.tiles.iter().find(|v| {
            let x=(p[0]-t.origin[0])/node-f64::from(v.column);
            let y=(p[1]-t.origin[1])/node-f64::from(v.row);
            (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)
        })).ok_or_else(||format!("far terrain: level {} missing node at {},{}",t.level,p[0],p[1]))?;
    if tile.heights.len()!=132*132 {return Err("far terrain: incomplete 132-square height raster".into());}
    let uv=std::array::from_fn::<_,2,_>(|a|(p[a]-t.origin[a])/node*128.0
        -f64::from(if a==0 {tile.column}else{tile.row})*128.0+TerrainTile::CORE_AT as f64);
    let x=uv[0].floor().clamp(0.0,130.0) as usize;let y=uv[1].floor().clamp(0.0,130.0) as usize;
    let fx=(uv[0]-x as f64).clamp(0.0,1.0);let fy=(uv[1]-y as f64).clamp(0.0,1.0);
    let a=f64::from(tile.heights[y*132+x])*(1.0-fx)+f64::from(tile.heights[y*132+x+1])*fx;
    let b=f64::from(tile.heights[(y+1)*132+x])*(1.0-fx)+f64::from(tile.heights[(y+1)*132+x+1])*fx;
    Ok((a*(1.0-fy)+b*fy)*range*2f64.powi(-20)*2f64.powi(8-t.level as i32)+offset)
}
fn rectangle(rect:[f64;4],distance:f64)->[f64;4] {
    [rect[0]-distance,rect[1]-distance,rect[2]+distance,rect[3]+distance]
}
fn edge(rect:[f64;4],side:usize,step:f64)->Vec<[f64;2]> {
    let corners=[[rect[0],rect[1]],[rect[2],rect[1]],[rect[2],rect[3]],[rect[0],rect[3]]];
    let (a,b)=(corners[side],corners[(side+1)%4]);
    let length=(b[0]-a[0]).abs()+(b[1]-a[1]).abs();let n=(length/step).ceil().max(1.0) as usize;
    (0..=n).map(|i|std::array::from_fn(|k|a[k]+(b[k]-a[k])*i as f64/n as f64)).collect()
}
/// Both directed polylines include their corners. Emit triangles without a T junction even when subdivisions differ.
fn zipper(a:&[[f64;2]],b:&[[f64;2]])->Vec<[[f64;2];3]> {
    let(mut i,mut j)=(0,0);let mut out=Vec::new();
    while i+1<a.len() || j+1<b.len() {
        let u=if i+1<a.len(){(i+1) as f64/(a.len()-1) as f64}else{f64::INFINITY};
        let v=if j+1<b.len(){(j+1) as f64/(b.len()-1) as f64}else{f64::INFINITY};
        if u<=v {out.push([a[i],b[j],a[i+1]]);i+=1;} else {out.push([a[i],b[j],b[j+1]]);j+=1;}
    }
    out
}
fn clip(input:&[[f64;3]],axis:usize,boundary:f64,greater:bool)->Vec<[f64;3]> {
    if input.is_empty(){return Vec::new();}
    let inside=|p:[f64;3]|if greater {p[axis]>=boundary}else{p[axis]<=boundary};
    let mut out=Vec::new();let mut previous=*input.last().expect("nonempty");
    for &p in input {
        if inside(previous)!=inside(p) {
            let t=(boundary-previous[axis])/(p[axis]-previous[axis]);
            let mut at=std::array::from_fn(|k|previous[k]+(p[k]-previous[k])*t);at[axis]=boundary;out.push(at);
        }
        if inside(p){out.push(p);} previous=p;
    }
    out
}
struct Piece { positions:Vec<[f32;3]>,normals:Vec<[f32;3]>,uvs:Vec<[f32;2]> }
impl Piece {fn new()->Self{Self{positions:Vec::new(),normals:Vec::new(),uvs:Vec::new()}}}

impl LowerAnvilFarTerrain {
    fn build(&self,world:&World,packet:&AnvilFarTerrain)->Result<BTreeMap<(u32,u32),Piece>,String> {
        let near=world.get::<AnvilTerrain>(packet.near).ok_or("far ring: near terrain must still be present")?;
        if near.level!=8 {return Err("far ring requires the established level-8 near height scale".into());}
        let coarse=packet.levels.iter().find(|t|t.level==5).ok_or("far ring: no level-5 colour/height nodes")?;
        let choose=|d:f64|if d<256.0 {5}else if d<640.0 {4}else{3};
        let height=|p:[f64;2],d:f64|->Result<f64,String> {
            let level=choose(d);
            let t=packet.levels.iter().find(|t|t.level==level).ok_or_else(||format!("far ring: no level {level}"))?;
            let h=sample(t,p,self.range,self.offset)?;
            if d>=self.seam {return Ok(h);}
            let border=[p[0].clamp(packet.rect[0],packet.rect[2]),p[1].clamp(packet.rect[1],packet.rect[3])];
            let n=sample(near,border,self.range,self.offset)?;
            let weight=(d/self.seam).clamp(0.0,1.0);let weight=weight*weight*(3.0-2.0*weight);
            Ok(n*(1.0-weight)+h*weight)
        };
        let mut pieces=BTreeMap::new();let node=coarse.side/32.0;
        let distance=|p:[f64;2]|(packet.rect[0]-p[0]).max(p[0]-packet.rect[2]).max(packet.rect[1]-p[1]).max(p[1]-packet.rect[3]).max(0.0);
        let mut inner=0.0;let mut inner_step=spacing(near);
        while inner<packet.radius {
            let step=if inner<256.0 {4.0}else if inner<640.0 {8.0}else if inner<1000.0 {16.0}else if inner<2000.0 {32.0}else{64.0};
            let next_break=if inner<256.0 {256.0}else if inner<640.0 {640.0}else if inner<1000.0 {1000.0}else if inner<2000.0 {2000.0}else{packet.radius};
            let outer=(inner+step).min(next_break).min(packet.radius);
            for side in 0..4 {
                let a=edge(rectangle(packet.rect,inner),side,inner_step);
                let b=edge(rectangle(packet.rect,outer),side,step);
                for triangle in zipper(&a,&b) {
                    // A shared ring always has one height function on both incident strips, including LOD breaks.
                    let mut points=[[0.0;3];3];
                    for k in 0..3 {points[k]=[triangle[k][0],triangle[k][1],height(triangle[k],distance(triangle[k]))?];}
                    let bounds=[points.iter().map(|p|p[0]).fold(f64::INFINITY,f64::min),
                        points.iter().map(|p|p[1]).fold(f64::INFINITY,f64::min),
                        points.iter().map(|p|p[0]).fold(f64::NEG_INFINITY,f64::max),
                        points.iter().map(|p|p[1]).fold(f64::NEG_INFINITY,f64::max)];
                    let col=|x:f64|((x-coarse.origin[0])/node).floor() as i64;
                    let row=|y:f64|((y-coarse.origin[1])/node).floor() as i64;
                    for r in row(bounds[1])..=row(bounds[3]) {for c in col(bounds[0])..=col(bounds[2]) {
                        if c<0 || r<0 || c>=32 || r>=32 {continue;}
                        let x=coarse.origin[0]+c as f64*node;let y=coarse.origin[1]+r as f64*node;
                        let mut polygon=points.to_vec();
                        for (axis,limit,greater) in [(0,x,true),(0,x+node,false),(1,y,true),(1,y+node,false)] {
                            polygon=clip(&polygon,axis,limit,greater);
                        }
                        if polygon.len()<3 {continue;}
                        if !coarse.tiles.iter().any(|t|t.column==c as u32 && t.row==r as u32) {
                            return Err(format!("far ring: colour node {c},{r} missing"));
                        }
                        let piece=pieces.entry((r as u32,c as u32)).or_insert_with(Piece::new);
                        for i in 1..polygon.len()-1 {
                            let face=[polygon[0],polygon[i],polygon[i+1]];
                            let ab=std::array::from_fn::<_,3,_>(|k|face[1][k]-face[0][k]);
                            let ac=std::array::from_fn::<_,3,_>(|k|face[2][k]-face[0][k]);
                            let normal=[ab[1]*ac[2]-ab[2]*ac[1],ab[2]*ac[0]-ab[0]*ac[2],ab[0]*ac[1]-ab[1]*ac[0]];
                            let length=normal.iter().map(|v|v*v).sum::<f64>().sqrt();if length<1e-9 {continue;}
                            for p in face {
                                piece.positions.push(near.frame.point(p.map(|v|v as f32)));
                                piece.normals.push(near.frame.direction(normal.map(|v|(v/length) as f32)));
                                piece.uvs.push([(((p[0]-x)/node*128.0+2.5)/132.0) as f32,
                                    (((p[1]-y)/node*128.0+2.5)/132.0) as f32]);
                            }
                        }
                    }}
                }
            }
            inner=outer;inner_step=step;
        }
        Ok(pieces)
    }
}
impl Pass for LowerAnvilFarTerrain {
    fn name(&self)-> &'static str {"lower-anvil-far-terrain"}
    fn options(&self)->Options{Options::from([("range".into(),self.range.to_string()),("offset".into(),self.offset.to_string()),("seam".into(),self.seam.to_string())])}
    fn may_lack_inputs(&self)->bool{true}
    fn requires(&self)->&[&'static str]{&[AnvilFarTerrain::NAME,AnvilTerrain::NAME]}
    fn provides(&self)->&[&'static str]{&[Mesh::NAME,Texture::NAME,PbrMaterial::NAME,Transform::NAME,Name::NAME,AnvilRenderStatus::NAME]}
    fn removes(&self)->&[&'static str]{&[AnvilFarTerrain::NAME]}
    fn approximates(&self)-> &'static str {"static rectangular terrain rings, 4/8/16/32/64 metre tessellation; stated seam morph; source far-colour tiles, no near detail outside the kept area"}
    fn run(&self,world:&mut World,log:&mut PassLog)->Result<(),String>{
        let ids:Vec<_>=world.iter::<AnvilFarTerrain>().map(|(e,_)|e).collect();
        for entity in ids {
            let packet=world.get::<AnvilFarTerrain>(entity).ok_or("far packet missing")?;
            let pieces=match self.build(world,packet){Ok(p)=>p,Err(reason)=>{
                world.insert(entity,AnvilRenderStatus{source:"coarse TNL ground".into(),template:None,reason:reason.clone(),unsupported:true});
                log.omit(&reason,1);continue;
            }};
            let packet=world.remove::<AnvilFarTerrain>(entity).ok_or("far packet missing")?;
            let coarse=packet.levels.iter().find(|t|t.level==5).ok_or("far colour level missing")?;
            let mut primitives=Vec::new();
            for ((row,column),piece) in pieces {
                let tile=coarse.tiles.iter().find(|t|t.row==row && t.column==column).ok_or("far colour tile missing")?;
                if tile.colour.len()!=33*33*8 {return Err(format!("far colour node {row},{column}: invalid BC1 payload"));}
                let texture=world.spawn();world.insert(texture,Texture{width:132,height:132,format:TexFormat::Bc1,
                    levels:vec![tile.colour.clone()],color_space:ColorSpace::Srgb,role:TexRole::Color,normal:None});
                let material=world.spawn();world.insert(material,PbrMaterial{base_color_texture:Some(TextureRef{
                    wrap:[Wrap::Clamp;2],..TextureRef::new(texture)}),metallic:0.0,roughness:1.0,..Default::default()});
                log.count("far ground triangles",piece.positions.len() as u64/3);
                primitives.push(Primitive{topology:Topology::Triangles,indices:None,material:Some(material),streams:BTreeMap::from([
                    (Semantic::Position,Stream::F32x3(piece.positions)),(Semantic::Normal,Stream::F32x3(piece.normals)),
                    (Semantic::TexCoord(0),Stream::F32x2(piece.uvs))])});
            }
            world.insert(entity,Mesh::new(primitives));world.insert(entity,Transform::IDENTITY);
            world.insert(entity,Name("Far ground: coarse terrain with stitched near seam".into()));
            world.insert(entity,AnvilRenderStatus{source:"TNL levels 5/4/3".into(),template:None,unsupported:false,
                reason:format!("{} m far-ground ring; exact level-8 inner edge, {} m height morph; source block-1 far colour, not near splat",packet.radius,self.seam)});
            log.approximate("far ground colour is the stored far tile; near splat detail ends at the near rectangle",1);
        }
        Ok(())
    }
}
#[cfg(test)]mod tests{
    use super::*;
    #[test]fn zipper_has_every_inner_and_outer_edge_without_a_t_junction(){
        let a=edge([0.0,0.0,8.0,8.0],0,0.5);let b=edge([-4.0,-4.0,12.0,12.0],0,4.0);
        let triangles=zipper(&a,&b);assert_eq!(triangles.len(),a.len()+b.len()-2);
        for pair in a.windows(2){assert!(triangles.iter().any(|t|t.contains(&pair[0])&&t.contains(&pair[1])));}
        for pair in b.windows(2){assert!(triangles.iter().any(|t|t.contains(&pair[0])&&t.contains(&pair[1])));}
    }
    #[test]fn coarse_height_restores_the_level_scale(){
        let t=AnvilTerrain{frame:primase_core::frame::SourceFrame::ENU,level:5,origin:[0.0;2],side:16384.0,
            tiles:vec![TerrainTile{row:0,column:0,heights:vec![1024;132*132],colour:vec![],material:vec![],block_8:vec![]}],
            materials:vec![],details:vec![],noise:None};
        assert_eq!(sample(&t,[10.0,10.0],5000.0,0.0).unwrap(),39.0625);
    }
    #[test]fn colour_clipping_preserves_the_height_on_an_intersected_edge(){
        let p=clip(&[[0.0,0.0,10.0],[2.0,0.0,20.0],[0.0,2.0,10.0]],0,1.0,false);
        assert!(p.iter().any(|v|v==&[1.0,0.0,15.0]));
    }
}
