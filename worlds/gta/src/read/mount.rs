//! The game's mount data: the pack list (`dlclist.xml`), each pack's `setup2.xml` and content file, extra folder
//! mounts and title-update mounts. Read strictly (every element must be one the game's files use); items tagged
//! for other platforms are left out (`platform="ps5|xbsx"`: the files they name are absent from PC installs).
use crate::read::xml::{Element, parse};
use crate::{Result, invalid};

/// The platform name items are matched against (`x64`, as `%PLATFORM%` expands on PC).
pub const PLATFORM: &str = "x64";

fn for_this_platform(e: &Element) -> bool {
    e.attr("platform")
        .is_none_or(|p| p.split('|').any(|x| x.trim().eq_ignore_ascii_case(PLATFORM)))
}

fn items(e: Option<&Element>) -> impl Iterator<Item = &Element> {
    e.into_iter()
        .flat_map(|e| e.items())
        .filter(|i| for_this_platform(i))
}

fn texts(e: Option<&Element>) -> Vec<String> {
    items(e)
        .map(|i| i.text.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

fn flag(e: &Element, name: &str, what: &str) -> Result<bool> {
    match e.child_value(name) {
        None => Ok(false),
        Some(v) if v.eq_ignore_ascii_case("true") => Ok(true),
        Some(v) if v.eq_ignore_ascii_case("false") => Ok(false),
        Some(v) => Err(invalid(
            "XML",
            0,
            format!("{what}: <{name} value=\"{v}\"> is not a boolean"),
        )),
    }
}

fn int(e: &Element, name: &str, what: &str) -> Result<i64> {
    match e.child_value(name) {
        None => Ok(0),
        Some(v) => v.trim().parse().map_err(|_| {
            invalid(
                "XML",
                0,
                format!("{what}: <{name} value=\"{v}\"> is not an integer"),
            )
        }),
    }
}

/// `dlclist.xml`: the packs the game looks for, in list order, as `platform:/dlcpacks/<name>/` paths, lowercase.
pub fn pack_list(bytes: &[u8]) -> Result<Vec<String>> {
    let what = "dlclist.xml";
    let r = parse(bytes, what)?;
    r.only(what, &["Paths"])?;
    let paths = r.child("Paths");
    if let Some(p) = paths {
        p.only(what, &["Item"])?;
    }
    Ok(texts(paths)
        .into_iter()
        .map(|t| t.replace('\\', "/").to_lowercase())
        .collect())
}

/// A pack's `setup2.xml`.
#[derive(Clone, Debug, Default)]
pub struct Setup {
    pub device_name: String,
    pub dat_file: String,
    pub name_hash: String,
    pub kind: String,
    pub order: i64,
    pub minor_order: i64,
    pub sub_pack_count: i64,
    pub level_pack: bool,
    /// Change-set groups in order: group name, change set names.
    pub groups: Vec<(String, Vec<String>)>,
}

pub fn setup(bytes: &[u8], what: &str) -> Result<Setup> {
    let r = parse(bytes, what)?;
    r.only(
        what,
        &[
            "deviceName",
            "datFile",
            "timeStamp",
            "nameHash",
            "contentChangeSetGroups",
            "contentChangeSets",
            "type",
            "order",
            "minorOrder",
            "subPackCount",
            "isLevelPack",
            "dependencyPackHash",
            "requiredVersion",
            "scriptCallstackSize",
            "startupScript",
            "patchPackPath",
        ],
    )?;
    let mut groups = Vec::new();
    if let Some(g) = r.child("contentChangeSetGroups") {
        g.only(what, &["Item"])?;
        for item in items(Some(g)) {
            item.only(what, &["NameHash", "ContentChangeSets"])?;
            groups.push((
                item.child_text("NameHash").unwrap_or_default().to_string(),
                texts(item.child("ContentChangeSets")),
            ));
        }
    }
    Ok(Setup {
        device_name: r.child_text("deviceName").unwrap_or_default().to_string(),
        dat_file: r.child_text("datFile").unwrap_or_default().to_string(),
        name_hash: r.child_text("nameHash").unwrap_or_default().to_string(),
        kind: r.child_text("type").unwrap_or_default().to_string(),
        order: int(&r, "order", what)?,
        minor_order: int(&r, "minorOrder", what)?,
        sub_pack_count: int(&r, "subPackCount", what)?,
        level_pack: flag(&r, "isLevelPack", what)?,
        groups,
    })
}

/// One data file of a content file.
#[derive(Clone, Debug, Default)]
pub struct DataFile {
    /// As written (`dlcMPHeist:/%PLATFORM%/levels/...rpf`).
    pub filename: String,
    pub file_type: String,
    pub overlay: bool,
    pub disabled: bool,
}

/// A change to the map data, inside a change set.
#[derive(Clone, Debug, Default)]
pub struct MapChange {
    pub associated_map: String,
    pub files_to_enable: Vec<String>,
    pub files_to_disable: Vec<String>,
    pub files_to_invalidate: Vec<String>,
}

/// A content change set.
#[derive(Clone, Debug, Default)]
pub struct ChangeSet {
    pub name: String,
    pub files_to_enable: Vec<String>,
    pub files_to_disable: Vec<String>,
    pub files_to_invalidate: Vec<String>,
    pub map_changes: Vec<MapChange>,
    /// `$level=<name>` from its execution conditions: the change set applies to that level only.
    pub level: Option<String>,
}

/// A pack's content file.
#[derive(Clone, Debug, Default)]
pub struct Content {
    pub data_files: Vec<DataFile>,
    pub change_sets: Vec<ChangeSet>,
}

pub fn content(bytes: &[u8], what: &str) -> Result<Content> {
    let r = parse(bytes, what)?;
    r.only(
        what,
        &[
            "disabledFiles",
            "includedXmlFiles",
            "includedDataFiles",
            "dataFiles",
            "contentChangeSets",
            "patchFiles",
        ],
    )?;
    let mut data_files = Vec::new();
    if let Some(d) = r.child("dataFiles") {
        d.only(what, &["Item"])?;
        for item in items(Some(d)) {
            item.only(
                what,
                &[
                    "filename",
                    "fileType",
                    "overlay",
                    "disabled",
                    "persistent",
                    "locked",
                    "loadCompletely",
                    "contents",
                    "patchFile",
                ],
            )?;
            data_files.push(DataFile {
                filename: item.child_text("filename").unwrap_or_default().to_string(),
                file_type: item.child_text("fileType").unwrap_or_default().to_string(),
                overlay: flag(item, "overlay", what)?,
                disabled: flag(item, "disabled", what)?,
            });
        }
    }
    let mut change_sets = Vec::new();
    if let Some(c) = r.child("contentChangeSets") {
        c.only(what, &["Item"])?;
        for item in items(Some(c)) {
            item.only(
                what,
                &[
                    "changeSetName",
                    "associatedMap",
                    "executionConditions",
                    "filesToDisable",
                    "filesToEnable",
                    "filesToInvalidate",
                    "mapChangeSetData",
                    "residentResources",
                    "txdToLoad",
                    "txdToUnload",
                    "unregisterResources",
                    "loadingScreenContext",
                    "requiresLoadingScreen",
                    "useCacheLoader",
                ],
            )?;
            let mut level = None;
            if let Some(ec) = item.child("executionConditions") {
                ec.only(what, &["activeChangesetConditions", "genericConditions"])?;
                if ec
                    .child("activeChangesetConditions")
                    .is_some_and(|a| !a.children.is_empty() || !a.text.trim().is_empty())
                {
                    return Err(invalid(
                        "XML",
                        0,
                        format!("{what}: active change set conditions are not read"),
                    ));
                }
                let g = ec.child_text("genericConditions").unwrap_or_default();
                if !g.is_empty() {
                    level = Some(
                        g.strip_prefix("$level=")
                            .ok_or_else(|| {
                                invalid("XML", 0, format!("{what}: generic condition '{g}' is not read"))
                            })?
                            .to_string(),
                    );
                }
            }
            let mut map_changes = Vec::new();
            if let Some(m) = item.child("mapChangeSetData") {
                m.only(what, &["Item"])?;
                for mi in items(Some(m)) {
                    mi.only(
                        what,
                        &[
                            "associatedMap",
                            "filesToDisable",
                            "filesToEnable",
                            "filesToInvalidate",
                            "residentResources",
                            "txdToLoad",
                            "txdToUnload",
                            "unregisterResources",
                        ],
                    )?;
                    map_changes.push(MapChange {
                        associated_map: mi.child_text("associatedMap").unwrap_or_default().to_string(),
                        files_to_enable: texts(mi.child("filesToEnable")),
                        files_to_disable: texts(mi.child("filesToDisable")),
                        files_to_invalidate: texts(mi.child("filesToInvalidate")),
                    });
                }
            }
            change_sets.push(ChangeSet {
                name: item.child_text("changeSetName").unwrap_or_default().to_string(),
                files_to_enable: texts(item.child("filesToEnable")),
                files_to_disable: texts(item.child("filesToDisable")),
                files_to_invalidate: texts(item.child("filesToInvalidate")),
                map_changes,
                level,
            });
        }
    }
    Ok(Content {
        data_files,
        change_sets,
    })
}

/// `extrafoldermount.xml`: folder mounts as (path, mounted as).
pub fn folder_mounts(bytes: &[u8], what: &str) -> Result<Vec<(String, String)>> {
    let r = parse(bytes, what)?;
    r.only(what, &["FolderMounts"])?;
    let mut out = Vec::new();
    if let Some(f) = r.child("FolderMounts") {
        f.only(what, &["Item"])?;
        for item in items(Some(f)) {
            item.only(what, &["path", "mountAs"])?;
            out.push((
                item.child_text("path").unwrap_or_default().to_string(),
                item.child_text("mountAs").unwrap_or_default().to_string(),
            ));
        }
    }
    Ok(out)
}

/// `extratitleupdatedata.meta`: title-update mounts as (device name, path).
pub fn title_update_mounts(bytes: &[u8], what: &str) -> Result<Vec<(String, String)>> {
    let r = parse(bytes, what)?;
    r.only(what, &["Mounts"])?;
    let mut out = Vec::new();
    if let Some(m) = r.child("Mounts") {
        m.only(what, &["Item"])?;
        for item in items(Some(m)) {
            item.only(what, &["deviceName", "path"])?;
            out.push((
                item.child_text("deviceName").unwrap_or_default().to_string(),
                item.child_text("path").unwrap_or_default().to_string(),
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_content_platforms_and_conditions() {
        let s = br#"<?xml version="1.0"?><SSetupData><deviceName>dlcX</deviceName><datFile>content.xml</datFile>
            <order value="7"/><subPackCount value="2"/><contentChangeSetGroups><Item><NameHash>GROUP_MAP</NameHash>
            <ContentChangeSets><Item>CS_A</Item><Item platform="ps5|xbsx">CS_CONSOLE</Item><item platform="x64">CS_PC</item>
            </ContentChangeSets></Item></contentChangeSetGroups></SSetupData>"#;
        let su = setup(s, "setup2.xml").expect("setup");
        assert_eq!(
            (su.device_name.as_str(), su.order, su.sub_pack_count),
            ("dlcX", 7, 2)
        );
        assert_eq!(
            su.groups,
            vec![(
                "GROUP_MAP".to_string(),
                vec!["CS_A".to_string(), "CS_PC".to_string()]
            )]
        );
        let c = br#"<CDataFileMgr__ContentsOfDataFileXml><dataFiles><Item><filename>dlcX:/%PLATFORM%/a.rpf</filename>
            <fileType>RPF_FILE</fileType><overlay value="true"/><disabled value="true"/></Item></dataFiles>
            <contentChangeSets><Item><changeSetName>CS_A</changeSetName><filesToEnable><Item>dlcX:/%PLATFORM%/a.rpf</Item>
            </filesToEnable><executionConditions><activeChangesetConditions/><genericConditions>$level=MO_JIM_L11</genericConditions>
            </executionConditions></Item><Item><changeSetName>CS_A</changeSetName><mapChangeSetData><Item><associatedMap>m</associatedMap>
            <filesToInvalidate><Item>platform:/levels/gta5/b.rpf</Item></filesToInvalidate></Item></mapChangeSetData></Item>
            </contentChangeSets></CDataFileMgr__ContentsOfDataFileXml>"#;
        let co = content(c, "content.xml").expect("content");
        assert!(co.data_files[0].overlay && co.data_files[0].disabled);
        assert_eq!(co.change_sets.len(), 2);
        assert_eq!(co.change_sets[0].level.as_deref(), Some("MO_JIM_L11"));
        assert_eq!(
            co.change_sets[1].map_changes[0].files_to_invalidate,
            vec!["platform:/levels/gta5/b.rpf"]
        );
        // unknown elements are refused
        assert!(setup(b"<SSetupData><mystery/></SSetupData>", "s").is_err());
        assert!(content(b"<CDataFileMgr__ContentsOfDataFileXml><dataFiles><Item><what/></Item></dataFiles></CDataFileMgr__ContentsOfDataFileXml>", "c").is_err());
    }
}
