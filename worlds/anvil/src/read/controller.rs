//! TimeOfDay/WeatherPropertyControllerData's registry-ordered entries (lighting.md §6–7).
//! The packed track header includes the first key's time in its upper half. No scanning of float payloads for
//! coincidental ClassIDs: each nested header, path node, key vector and trailer is consumed in order.
pub const TIME_OF_DAY:u32=0xF0C8_5018;
pub const WEATHER:u32=0x8870_04DC;
const ENTRY:u32=0x050A_3620;
const PATH:u32=0x3C00_B2E0;
const NODE:u32=0x1A87_50BA;
const FLOAT:u32=0xCFD6_6AAD;
const VECTOR:u32=0x8BE9_C203;
const BYTE:u32=0x5841_BD2B;

#[derive(Clone,Debug,PartialEq)]
pub struct Track {
    pub path:Vec<u32>,
    pub kind:u32,
    pub times:Vec<u16>,
    pub values:Vec<[f32;4]>,
    pub path_flags:[u8;2],
    pub binding:u64,
    pub trailer:[u8;5],
}
impl Track {
    pub fn at(&self,time:f32)->Result<[f32;4],String>{
        if !time.is_finite() || self.times.is_empty() || self.times.len()!=self.values.len(){return Err("controller: invalid sample/key vector".into());}
        if time<=f32::from(self.times[0]){return Ok(self.values[0]);}
        for i in 0..self.times.len()-1{
            if time<=f32::from(self.times[i+1]){
                if self.kind==BYTE{return Ok(self.values[i]);}
                let f=(time-f32::from(self.times[i]))/f32::from(self.times[i+1]-self.times[i]);
                return Ok(std::array::from_fn(|k|self.values[i][k]+(self.values[i+1][k]-self.values[i][k])*f));
            }
        }
        self.values.last().copied().ok_or("controller: no last key".into())
    }
}
#[derive(Clone,Debug,PartialEq)]
pub struct Controller {pub tracks:Vec<Track>}
struct Cursor<'a>{bytes:&'a[u8],at:usize}
impl<'a> Cursor<'a>{
    fn take<const N:usize>(&mut self)->Result<[u8;N],String>{
        let end=self.at.checked_add(N).ok_or("controller offset overflow")?;
        let value=self.bytes.get(self.at..end).and_then(|v|v.try_into().ok())
            .ok_or_else(||format!("controller truncated at {} reading {N} bytes",self.at))?;
        self.at=end;Ok(value)
    }
    fn u32(&mut self)->Result<u32,String>{Ok(u32::from_le_bytes(self.take()?))}
    fn header(&mut self,class:u32)->Result<(),String>{
        let _id=self.take::<8>()?;let found=self.u32()?;
        if found!=class{return Err(format!("controller: expected class {class:08x}, found {found:08x} at {}",self.at-4));}Ok(())
    }
}
impl Controller {
    pub fn parse(bytes:&[u8])->Result<Self,String>{
        let mut c=Cursor{bytes,at:0};
        if c.take::<1>()? != [1]{return Err("controller: unknown ManagedObject header".into());}
        let count=c.u32()? as usize;
        if count>8192{return Err("controller: entry count exceeds supported resource bound".into());}
        let mut tracks=Vec::with_capacity(count);
        for _ in 0..count{
            c.header(ENTRY)?;c.header(PATH)?;
            let nodes=c.u32()? as usize;
            if !(1..=32).contains(&nodes){return Err("controller: invalid property path length".into());}
            let mut path=Vec::with_capacity(nodes);
            for _ in 0..nodes{
                c.header(NODE)?;
                // Fixed scalar-path node profile; conditioned/indexed/solver nodes need their own grammar.
                if c.take::<2>()? != [255;2] || c.take::<8>()? != [0;8] || c.take::<3>()? != [1,0,0]{
                    return Err("controller: conditioned/indexed PropertyPathNode is not the scalar-path profile".into());
                }
                path.push(c.u32()?);
                if c.take::<25>()? != [0;25] || c.take::<1>()? != [3]{
                    return Err("controller: PropertyPathNode solver/index fields differ from the established profile".into());
                }
            }
            let path_flags=c.take()?;
            let binding=u64::from_le_bytes(c.take()?);
            let _track_id=c.take::<8>()?;let kind=c.u32()?;
            let width=match kind{FLOAT=>1,VECTOR=>4,BYTE=>1,_=>return Err(format!("controller: unsupported track class {kind:08x}"))};
            let packed=c.u32()?;let keys=(packed&65535) as usize;
            if !(1..=4096).contains(&keys){return Err("controller: invalid key count".into());}
            let mut times=Vec::with_capacity(keys);times.push((packed>>16) as u16);
            for _ in 1..keys{times.push(u16::from_le_bytes(c.take()?));}
            if times.windows(2).any(|p|p[0]>=p[1]){return Err("controller: key times are not strictly increasing".into());}
            let mut values=Vec::with_capacity(keys);
            for _ in 0..keys{
                let mut value=[0.0;4];
                for lane in value.iter_mut().take(width){
                    *lane=if kind==BYTE{f32::from(c.take::<1>()?[0])}else{f32::from_bits(c.u32()?)};
                    if !lane.is_finite(){return Err("controller: non-finite key".into());}
                }
                values.push(value);
            }
            tracks.push(Track{path,kind,times,values,path_flags,binding,trailer:c.take()?});
        }
        if c.at!=bytes.len(){return Err(format!("controller: {} trailing bytes",bytes.len()-c.at));}
        Ok(Self{tracks})
    }
    /// Match the complete suffix, rejecting duplicate candidates instead of choosing a fog/sky property by its
    /// final name alone (several cloud and fog objects each have Albedo and Density).
    pub fn value(&self,path:&[&str],time:f32)->Result<Option<[f32;4]>,String>{
        let ids:Vec<_>=path.iter().map(|s|primase_hash::crc32(s.as_bytes())).collect();
        let mut matches=self.tracks.iter().filter(|t|t.path.ends_with(&ids));
        let value=matches.next().map(|t|t.at(time)).transpose()?;
        if matches.next().is_some(){return Err(format!("controller: ambiguous property {}",path.join("/")));}
        Ok(value)
    }
}
#[cfg(test)]mod tests{
    use super::*;
    #[test]fn packed_first_time_is_preserved_and_noon_interpolates_the_curve(){
        let mut b=vec![1];b.extend(1u32.to_le_bytes());
        for class in [ENTRY,PATH]{b.extend([0;8]);b.extend(class.to_le_bytes());}
        b.extend(1u32.to_le_bytes());b.extend([0;8]);b.extend(NODE.to_le_bytes());
        b.extend([255;2]);b.extend([0;8]);b.extend([1,0,0]);b.extend(primase_hash::crc32(b"Density").to_le_bytes());
        b.extend([0;25]);b.push(3);b.extend([0;2]);b.extend([0;8]);b.extend([0;8]);b.extend(FLOAT.to_le_bytes());
        b.extend(((1200u32<<16)|2).to_le_bytes());b.extend(1680u16.to_le_bytes());
        b.extend(2f32.to_le_bytes());b.extend(4f32.to_le_bytes());b.extend([0;5]);
        let c=Controller::parse(&b).unwrap();assert_eq!(c.tracks[0].times,[1200,1680]);
        assert_eq!(c.value(&["Density"],1440.0).unwrap().unwrap()[0],3.0);
        b.push(0);assert!(Controller::parse(&b).is_err());
    }
}
