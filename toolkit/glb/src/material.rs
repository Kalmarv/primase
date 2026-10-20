//! A glTF document's sampler and material facts in core's terms, for readers that take materials from the JSON
//! themselves ([`crate::Gltf::json`]): a sampler's filters and wrap ([`sampler_filters`]), a material's shading
//! ([`material_shading`]). The glTF writer writes the same codes.

use primase_core::pbr::{Filter, Filters, MipFilter, Shading, Wrap};
use serde_json::Value;

use crate::Result;
use crate::json::{Obj, invalid};

/// A sampler object's filters and wrap along u then v (glTF 2.0 5.26): each filter as stated, `None` where the
/// sampler states none (`minFilter` states the filter within a level and across mip levels together); wrap
/// `REPEAT` where unstated, as glTF defines. An unknown code is an error naming it.
pub fn sampler_filters(sampler: &Value) -> Result<(Filters, [Wrap; 2])> {
    let s = Obj::new(sampler, "sampler")?;
    let mag = match s.u64_opt("magFilter")? {
        None => None,
        Some(9728) => Some(Filter::Nearest),
        Some(9729) => Some(Filter::Linear),
        Some(c) => return Err(invalid("sampler.magFilter", &format!("{c} is not 9728 or 9729"))),
    };
    let (min, mip) = match s.u64_opt("minFilter")? {
        None => (None, None),
        Some(9728) => (Some(Filter::Nearest), Some(MipFilter::None)),
        Some(9729) => (Some(Filter::Linear), Some(MipFilter::None)),
        Some(9984) => (Some(Filter::Nearest), Some(MipFilter::Nearest)),
        Some(9985) => (Some(Filter::Linear), Some(MipFilter::Nearest)),
        Some(9986) => (Some(Filter::Nearest), Some(MipFilter::Linear)),
        Some(9987) => (Some(Filter::Linear), Some(MipFilter::Linear)),
        Some(c) => {
            return Err(invalid(
                "sampler.minFilter",
                &format!("{c} is not a glTF minification filter"),
            ));
        }
    };
    let wrap = |key: &str| -> Result<Wrap> {
        match s.u64_opt(key)? {
            None | Some(10497) => Ok(Wrap::Repeat),
            Some(33071) => Ok(Wrap::Clamp),
            Some(33648) => Ok(Wrap::Mirror),
            Some(c) => Err(invalid(
                &format!("sampler.{key}"),
                &format!("{c} is not a glTF wrap mode"),
            )),
        }
    };
    Ok((Filters { mag, min, mip }, [wrap("wrapS")?, wrap("wrapT")?]))
}

/// A material object's shading: unlit when it carries `KHR_materials_unlit`, else lit.
pub fn material_shading(material: &Value) -> Result<Shading> {
    let m = Obj::new(material, "material")?;
    let unlit = m
        .get("extensions")
        .and_then(|x| x.get("KHR_materials_unlit"))
        .is_some();
    Ok(if unlit { Shading::Unlit } else { Shading::Lit })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_filter_code_reads_as_stated() {
        let (f, wrap) = sampler_filters(&json!({})).unwrap();
        assert_eq!((f, wrap), (Filters::default(), [Wrap::Repeat; 2]));
        let (f, wrap) =
            sampler_filters(&json!({"magFilter": 9728, "minFilter": 9984, "wrapS": 33071, "wrapT": 33648}))
                .unwrap();
        assert_eq!(
            f,
            Filters {
                mag: Some(Filter::Nearest),
                min: Some(Filter::Nearest),
                mip: Some(MipFilter::Nearest)
            }
        );
        assert_eq!(wrap, [Wrap::Clamp, Wrap::Mirror]);
        for (code, min, mip) in [
            (9728, Filter::Nearest, MipFilter::None),
            (9729, Filter::Linear, MipFilter::None),
            (9985, Filter::Linear, MipFilter::Nearest),
            (9986, Filter::Nearest, MipFilter::Linear),
            (9987, Filter::Linear, MipFilter::Linear),
        ] {
            let (f, _) = sampler_filters(&json!({ "minFilter": code })).unwrap();
            assert_eq!((f.mag, f.min, f.mip), (None, Some(min), Some(mip)), "{code}");
        }
        assert!(sampler_filters(&json!({"magFilter": 9984})).is_err());
        assert!(sampler_filters(&json!({"minFilter": 1})).is_err());
        assert!(sampler_filters(&json!({"wrapS": 2})).is_err());
    }

    #[test]
    fn unlit_is_the_extension() {
        assert_eq!(
            material_shading(&json!({"extensions": {"KHR_materials_unlit": {}}})).unwrap(),
            Shading::Unlit
        );
        assert_eq!(material_shading(&json!({"name": "m"})).unwrap(), Shading::Lit);
    }
}
