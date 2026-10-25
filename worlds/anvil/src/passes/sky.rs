//! Static source time/weather curves and authored scattering tables. The automatic elevation(hour) formula is
//! unknown: retain the source's manual elevation and report it, rather than calling a morning dump value noon.
use std::collections::BTreeMap;
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{PbrMaterial,Shading};
use primase_core::{Component,Mesh,Name,Primitive,Semantic,Stream,Topology,Transform,World,Light,LightKind};
use crate::components::{AnvilRenderStatus,AnvilSky,AnvilSkyControllers};

/// Temporary evaluated sky, not another world environment. Only the optional asset sphere needs its panorama;
/// consumers of the prepared world use core.environment and core.atmosphere.
struct StaticSky {
    description:String, width:u32, height:u32, radiance:Vec<[f32;4]>,
    towards_sun:[f32;3], sunlight:[f32;3], ambient:[f32;3],
    fog:Option<primase_core::Fog>, notices:Vec<String>,
}

#[derive(Clone,Debug)]
pub struct LowerAnvilSky {pub hour:f32,pub weather:f32,pub resolution:usize,pub sphere:bool}
pub const LOWER_ANVIL_SKY:PassSpec=PassSpec{
    name:"lower-anvil-sky",options:&[
        OptionSpec::required("hour","12","static hour; source key times are 30-second units"),
        OptionSpec::required("weather","0","source weather-curve input, 0 to 12000; forecast omitted"),
        OptionSpec::required("sun","stored","authored manual elevation; automatic SunPositionMode is explicitly unresolved"),
        OptionSpec::required("resolution","64","radiance panorama width, even, 8 to 512"),
        OptionSpec::required("preview","environment","environment: lighting/background only; sphere: also show the sky asset"),
    ],make:|g:&Given|{
        let hour=g.required("hour").parse::<f32>().map_err(|_|"hour: expected number")?;
        let weather=g.required("weather").parse::<f32>().map_err(|_|"weather: expected number")?;
        let resolution=g.required("resolution").parse::<usize>().map_err(|_|"resolution: expected integer")?;
        let sphere=match g.required("preview"){"environment"=>false,"sphere"=>true,_=>return Err("preview must be environment or sphere".into())};
        if g.required("sun")!="stored"{return Err("sun must be stored; the automatic elevation formula is not established".into());}
        if !hour.is_finite() || !(0.0..24.0).contains(&hour) || !weather.is_finite() || !(0.0..=12000.0).contains(&weather)
            || !(8..=512).contains(&resolution) || resolution%2!=0{return Err("invalid static sky choices".into());}
        Ok(Box::new(LowerAnvilSky{hour,weather,resolution,sphere}))
    },
};
impl LowerAnvilSky {
    fn environment(&self,source:&AnvilSky,controllers:Option<&AnvilSkyControllers>)->Result<StaticSky,String>{
        let sky=&source.stored;let time=self.hour*120.0;
        let value=|path:&[&str]|->Result<Option<[f32;4]>,String>{controllers.map(|c|c.time_of_day.value(path,time)).transpose().map(Option::flatten)};
        let scalar=|name:&str,fallback:f32|->Result<f32,String>{Ok(value(&["LayeredSky",name])?.map_or(fallback,|v|v[0]))};
        let mut notices=vec!["clouds, moon, stars, forecast and effect simulation omitted".into()];
        notices.push("source colours/intensities treated as relative linear radiance; photometric calibration and post-process exposure/grading not reproduced".into());
        notices.push("core.environment uses the scattering LUT's upper-hemisphere mean as flat ambient; directional sky radiance/reflections are approximated, not a retained source panorama".into());
        if controllers.is_none(){notices.push("time/weather controller absent: authored LayeredSky defaults retained".into());}
        let azimuth=scalar("SunLightAzimuth",sky.sun_azimuth)?;
        let tilt=scalar("SunTiltAngle",sky.sun_tilt)?;
        let parameter=scalar("SunLightElevation",sky.sun_elevation)?;
        let elevation=sky.sun_elevation;
        notices.push(format!("SunPositionMode {} at hour {} (controller tick {time}): automatic direction unresolved; PS_SunSkyModel/1815 consumes already evaluated cb4[9].xyz, not hour/tilt/elevation tracks. Authored manual elevation {elevation} degrees retained; controller azimuth {azimuth}, tilt {tilt}, elevation parameter {parameter}. Need the CPU writer of cb4[9], not a guessed astronomical formula",sky.sun_position_mode,self.hour));
        for path in [vec!["ColorGradingControllables","Parameters","Exposure"],
            vec!["HDRLightingSettings","MinEV"],vec!["HDRLightingSettings","MaxEV"],
            vec!["HDRLightingSettings","EmissiveCurve"]] {
            if let Some(v)=value(&path)? {
                notices.push(format!("{} = {} at controller tick {time}: source parameter retained; mapping to evaluated cb1[39].x is unresolved. cb1[39].x is a linear radiance multiplier (core stops = log2(scale)); grading Exposure is not assumed to be stops, MinEV/MaxEV are adaptation bounds, t23 is an emissive divisor",path.join("/"),v[0]));
            }
        }
        notices.push("core.atmosphere exposure unstated: the selected look has no resolved cb1[39].x frame multiplier; one historical snapshot with a multiplier of one does not establish this hour's exposure".into());
        let (az,el)=(azimuth.to_radians(),elevation.to_radians());
        let sun=[el.cos()*az.sin(),el.cos()*az.cos(),el.sin()];
        let colour=value(&["LayeredSky","SunlightColor"])?.unwrap_or(sky.sun_color);
        let intensity=scalar("SunlightIntensity",sky.sun_intensity)?;
        let sunlight=[colour[0]*intensity,colour[1]*intensity,colour[2]*intensity];
        let mie=[scalar("MieG",sky.mie[0])?,scalar("MieGPeak",sky.mie[1])?,scalar("MieGPeakBlend",sky.mie[2])?];
        if sunlight.iter().any(|v|!v.is_finite() || *v<0.0){return Err("sky: invalid source sunlight".into());}
        if let Some(c)=controllers.and_then(|c|c.weather.as_ref()){
            for track in &c.tracks{
                let sampled=track.at(self.weather)?;
                notices.push(format!("weather path {} = {sampled:?}; retained, cloud/rain simulation omitted",track.path.iter().map(|v|format!("{v:08x}")).collect::<Vec<_>>().join("/")));
            }
        }
        let fog=match(value(&["Fog","VolumetricFog","Albedo"])?,value(&["Fog","VolumetricFog","Density"])?,
            value(&["Fog","VolumetricFog","Bottom"])?,value(&["Fog","VolumetricFog","Top"])?,
            value(&["Fog","VolumetricFog","PhaseFunction"])?) {
            (Some(a),Some(d),Some(bottom),Some(top),Some(phase))=>{
                if d[0]<0.0 || bottom[0]>=top[0] || a[..3].iter().any(|v|*v<0.0){return Err("sky: invalid source fog slab".into());}
                notices.push(format!("source volumetric fog: density treated as inverse metres, ENU altitude bounds as core Y heights, albedo times hemisphere ambient as constant in-scatter; phase {} and dense-fog noise, shadows and multiple scattering omitted",phase[0]));
                Some(primase_core::Fog{colour:[a[0],a[1],a[2]],falloff:primase_core::FogFalloff::ExponentialHeightSlab{
                    density:d[0],bottom:bottom[0],top:top[0]}})
            },
            (None,None,None,None,None)=>None,
            _=>return Err("sky: incomplete Fog/VolumetricFog controller inputs".into()),
        };
        let width=self.resolution;let height=width/2;let mut radiance=Vec::with_capacity(width*height);
        let mut ambient=[0.0;3];let mut weight=0.0;
        for y in 0..height{
            let el=(0.5-(y as f32+0.5)/height as f32)*std::f32::consts::PI;
            for x in 0..width{
                let az=((x as f32+0.5)/width as f32-0.5)*std::f32::consts::TAU;
                let core=[el.cos()*az.sin(),el.sin(),-el.cos()*az.cos()];
                let direction=primase_core::frame::SourceFrame::ENU.direction_to_source(core);
                let rgb=sky.radiance(self.hour,direction,sun,mie)?.map(|v|v.max(0.0));
                let rgb=std::array::from_fn::<_,3,_>(|k|rgb[k]*sunlight[k]);
                if core[1]>0.0{let w=core[1]*el.cos();for k in 0..3{ambient[k]+=rgb[k]*w;}weight+=w;}
                radiance.push([rgb[0],rgb[1],rgb[2],1.0]);
            }
        }
        if weight>0.0{ambient=ambient.map(|v|v/weight);}
        let fog=fog.map(|mut fog|{fog.colour=std::array::from_fn(|k|fog.colour[k]*ambient[k]);fog});
        Ok(StaticSky{description:format!("{} at {}h: source scattering + static time/weather curves; authored manual sun elevation",
            controllers.map_or("LayeredSky",|c|c.look.as_str()),self.hour),
            width:width as u32,height:height as u32,radiance,
            towards_sun:primase_core::frame::SourceFrame::ENU.direction(sun),sunlight,ambient,fog,notices})
    }
}

impl Pass for LowerAnvilSky{
    fn name(&self)-> &'static str{"lower-anvil-sky"}
    fn options(&self)->Options{Options::from([("hour".into(),self.hour.to_string()),("weather".into(),self.weather.to_string()),
        ("sun".into(),"stored".into()),("resolution".into(),self.resolution.to_string()),
        ("preview".into(),if self.sphere{"sphere"}else{"environment"}.into())])}
    fn may_lack_inputs(&self)->bool{true}
    fn requires(&self)->&[&'static str]{&[AnvilSky::NAME]}
    fn provides(&self)->&[&'static str]{&["core.environment","core.atmosphere",AnvilRenderStatus::NAME,Mesh::NAME,PbrMaterial::NAME,Transform::NAME,Name::NAME,Light::NAME,primase_core::Source::NAME]}
    fn modifies(&self)->&[&'static str]{&["core.environment","core.atmosphere",AnvilRenderStatus::NAME]}
    fn approximates(&self)-> &'static str{"static source defaults; authored manual sun elevation (automatic mode unresolved); no clouds, moon, stars, forecast, or volumetric fog simulation"}
    fn run(&self,world:&mut World,log:&mut PassLog)->Result<(),String>{
        let ids:Vec<_>=world.iter::<AnvilSky>().map(|(e,_)|e).collect();
        for entity in ids{
            let source=world.get::<AnvilSky>(entity).ok_or("sky missing")?;
            let environment=match self.environment(source,world.get::<AnvilSkyControllers>(entity)){
                Ok(e)=>e,Err(reason)=>{world.insert(entity,AnvilRenderStatus{source:"LayeredSky".into(),template:None,reason:reason.clone(),unsupported:true});log.omit(&reason,1);continue;}
            };
            for notice in &environment.notices{log.approximate(notice,1);}
            // Publish one set of established meanings for every consumer; the source data stays on AnvilSky.
            let sun=world.spawn();
            world.insert(sun,Name("LayeredSky static source sun (authored manual elevation)".into()));
            world.insert(sun,primase_core::Source(environment.description.clone()));
            world.insert(sun,Light{kind:LightKind::Directional,color:environment.sunlight,intensity:1.0,
                range:None,cast_shadows:None});
            world.insert(sun,Transform::facing([0.0;3],environment.towards_sun.map(|v|-v))
                .ok_or("sky: invalid authored sun direction")?);
            world.insert_resource(primase_core::Environment{sun:Some(sun),ambient:Some(primase_core::Ambient{
                intensity:1.0,light:primase_core::AmbientLight::Flat(environment.ambient)})});
            // No exposure track's units have been established for Anvil: leave exposure unstated, not guessed.
            world.insert_resource(primase_core::Atmosphere{fog:environment.fog.clone(),exposure:None,volumetric:None});
            if self.sphere{
                let material=world.spawn();world.insert(material,PbrMaterial{base_color:[1.0;4],metallic:0.0,roughness:1.0,
                    double_sided:true,shading:Shading::Unlit,..Default::default()});
                let(mut positions,mut normals,mut colours,mut indices)=(Vec::new(),Vec::new(),Vec::new(),Vec::new());
                for y in 0..=environment.height{for x in 0..=environment.width{
                    let el=(0.5-y as f32/environment.height as f32)*std::f32::consts::PI;
                    let az=(x as f32/environment.width as f32-0.5)*std::f32::consts::TAU;
                    let d=[el.cos()*az.sin(),el.sin(),-el.cos()*az.cos()];positions.push(d.map(|v|v*10.0));normals.push(d);
                    let px=x.min(environment.width-1);let py=y.min(environment.height-1);
                    colours.push(environment.radiance[(py*environment.width+px) as usize]);
                }}
                for y in 0..environment.height{for x in 0..environment.width{
                    let a=y*(environment.width+1)+x;let b=a+environment.width+1;indices.extend([a,b,a+1,a+1,b,b+1]);
                }}
                world.insert(entity,Mesh::new(vec![Primitive{topology:Topology::Triangles,indices:Some(indices),material:Some(material),
                    streams:BTreeMap::from([(Semantic::Position,Stream::F32x3(positions)),(Semantic::Normal,Stream::F32x3(normals)),
                        (Semantic::Color(0),Stream::F32x4(colours))])}]));world.insert(entity,Transform::IDENTITY);
            }
            world.insert(entity,AnvilRenderStatus{source:environment.description.clone(),template:None,unsupported:false,reason:environment.notices.join("; ")});
            log.count("source static sky environments",1);
        }
        Ok(())
    }
}

#[cfg(test)]mod tests{
    use super::*;
    use crate::read::sky::{SkyKeyframe,StoredSky};
    use crate::read::controller::{Controller,Track};
    fn source()->AnvilSky{
        AnvilSky{index_id:7,stored:StoredSky{sun_position_mode:0,sun_azimuth:30.0,sun_tilt:0.0,
            sun_elevation:30.0,sun_color:[1.0;4],sun_intensity:1.0,mie:[0.5,0.99,0.1],
            keyframes:vec![SkyKeyframe{hour:12.0,crc:0,inscatter:[0u8,0x38].repeat(65536),
                optical_depth:vec![],irradiance:vec![],sun_depth:vec![],sun_irradiance:vec![]}]}}
    }
    fn track(path:&[&str],value:[f32;4])->Track{
        Track{path:path.iter().map(|s|primase_hash::crc32(s.as_bytes())).collect(),kind:0x8BE9_C203,
            times:vec![0,2880],values:vec![value,value],path_flags:[0;2],binding:0,trailer:[0;5]}
    }
    #[test]fn source_controller_colour_and_fog_reach_the_core_resources(){
        let controls=AnvilSkyControllers{look:"fixture".into(),weather:None,time_of_day:Controller{tracks:vec![
            track(&["LayeredSky","SunlightColor"],[1.0,0.5,0.25,1.0]),
            track(&["LayeredSky","SunLightElevation"],[0.317,0.0,0.0,0.0]),
            track(&["ColorGradingControllables","Parameters","Exposure"],[0.8,0.0,0.0,0.0]),
            track(&["HDRLightingSettings","MinEV"],[-7.909,0.0,0.0,0.0]),
            track(&["Fog","VolumetricFog","Albedo"],[0.6,0.8,1.0,1.0]),
            track(&["Fog","VolumetricFog","Density"],[0.001,0.0,0.0,0.0]),
            track(&["Fog","VolumetricFog","Bottom"],[10.0,0.0,0.0,0.0]),
            track(&["Fog","VolumetricFog","Top"],[1000.0,0.0,0.0,0.0]),
            track(&["Fog","VolumetricFog","PhaseFunction"],[0.3,0.0,0.0,0.0]) ]}};
        let pass=LowerAnvilSky{hour:12.0,weather:0.0,resolution:8,sphere:false};
        let env=pass.environment(&source(),Some(&controls)).unwrap();
        assert_eq!(env.sunlight,[1.0,0.5,0.25]);assert_eq!(env.radiance.len(),32);
        assert!(matches!(env.fog.as_ref().unwrap().falloff,primase_core::FogFalloff::ExponentialHeightSlab{density,bottom:10.0,top:1000.0} if density==0.001));
        assert!(env.ambient.iter().all(|v|v.is_finite() && *v>0.0));
        assert!((env.towards_sun[1]-0.5).abs()<1e-6,"0.317 is a controller parameter, not elevation");
        assert!(env.notices.iter().any(|s|s.contains("SunPositionMode")));
        assert!(env.notices.iter().any(|s|s.contains("Parameters/Exposure = 0.8") && s.contains("not assumed to be stops")));
        assert!(env.notices.iter().any(|s|s.contains("PS_SunSkyModel/1815") && s.contains("cb4[9].xyz")));
        let mut world=World::new();let e=world.spawn();world.insert(e,source());world.insert(e,controls);
        pass.run(&mut world,&mut PassLog::default()).unwrap();
        let environment=world.resource::<primase_core::Environment>().unwrap();
        assert_eq!(world.get::<Light>(environment.sun.unwrap()).unwrap().color,[1.0,0.5,0.25]);
        let atmosphere=world.resource::<primase_core::Atmosphere>().unwrap();
        assert_eq!(atmosphere.fog,env.fog);
        assert!(atmosphere.exposure.is_none(),"no resolved frame multiplier is available for this look");
    }
    #[test]fn scattering_interpolation_wraps_midnight(){
        let mut source=source();let bright=source.stored.keyframes[0].clone();
        let mut dark=bright.clone();dark.hour=23.0;dark.inscatter.fill(0);
        let mut bright=bright;bright.hour=1.0;
        let sun=[0.0,0.0,1.0];let d=[1.0,0.0,0.0];
        let expected=bright.radiance(d,sun,source.stored.mie).unwrap();
        source.stored.keyframes=vec![dark,bright];
        let actual=source.stored.radiance(0.0,d,sun,source.stored.mie).unwrap();
        for k in 0..3{assert!((actual[k]-expected[k]*0.5).abs()<1e-7);}
    }
}
