//! Hand-written miniature Havok 7.1.0-r1 packfiles: every value is worked out here by hand (no game data, no second
//! reader), so these check the packfile and physics views against fixtures. Real-data gates for the 490 SHRED files live in
//! that game's crate (`worlds/shred/tests/collision_disc.rs`).
#![allow(clippy::unwrap_used)]
use primase_havok::packfile::Packfile;
use primase_havok::packfile::physics::{self, ConvexVertices, ExtendedMesh, MOTION_FIXED, RigidBody, Shape};

/// Builds a packfile: class names, data bytes, local/global/virtual fixups.
struct Builder {
    classes: Vec<&'static str>,
    data: Vec<u8>,
    local: Vec<(u32, u32)>,
    global: Vec<(u32, u32)>,
    objects: Vec<(u32, usize)>,
}

impl Builder {
    fn new(size: usize) -> Self {
        Self {
            classes: Vec::new(),
            data: vec![0; size],
            local: Vec::new(),
            global: Vec::new(),
            objects: Vec::new(),
        }
    }
    fn class(&mut self, name: &'static str) -> usize {
        self.classes.iter().position(|c| *c == name).unwrap_or_else(|| {
            self.classes.push(name);
            self.classes.len() - 1
        })
    }
    fn object(&mut self, offset: u32, class: &'static str) {
        let c = self.class(class);
        self.objects.push((offset, c));
    }
    fn u32(&mut self, at: usize, v: u32) {
        self.data[at..at + 4].copy_from_slice(&v.to_be_bytes());
    }
    fn u16(&mut self, at: usize, v: u16) {
        self.data[at..at + 2].copy_from_slice(&v.to_be_bytes());
    }
    fn f32(&mut self, at: usize, v: f32) {
        self.data[at..at + 4].copy_from_slice(&v.to_be_bytes());
    }
    fn floats(&mut self, at: usize, v: &[f32]) {
        for (i, x) in v.iter().enumerate() {
            self.f32(at + 4 * i, *x);
        }
    }
    /// A pointer slot with a local fixup.
    fn local(&mut self, at: usize, to: usize) {
        self.local.push((at as u32, to as u32));
    }
    /// A pointer slot with a global fixup.
    fn global(&mut self, at: usize, to: usize) {
        self.global.push((at as u32, to as u32));
    }
    /// An hkArray header at `at`, its data at `to` (local fixup), `len` elements.
    fn array(&mut self, at: usize, to: Option<usize>, len: u32) {
        if let Some(to) = to {
            self.local(at, to);
        }
        self.u32(at + 4, len);
        self.u32(at + 8, 0x8000_0000 | len);
    }
    fn build(&self) -> Vec<u8> {
        let mut names = Vec::new();
        let mut name_at = Vec::new();
        for c in &self.classes {
            names.extend(0x1234_5678u32.to_be_bytes());
            names.push(9);
            name_at.push(names.len() as u32);
            names.extend(c.as_bytes());
            names.push(0);
        }
        names.extend([0xFF; 3]);
        let data_len = self.data.len() as u32;
        let mut section = self.data.clone();
        let local_at = data_len;
        for (s, d) in &self.local {
            section.extend(s.to_be_bytes());
            section.extend(d.to_be_bytes());
        }
        section.extend([0xFF; 8]);
        let global_at = section.len() as u32;
        for (s, d) in &self.global {
            section.extend(s.to_be_bytes());
            section.extend(2u32.to_be_bytes());
            section.extend(d.to_be_bytes());
        }
        section.extend([0xFF; 12]);
        let virtual_at = section.len() as u32;
        for (o, c) in &self.objects {
            section.extend(o.to_be_bytes());
            section.extend(0u32.to_be_bytes());
            section.extend(name_at[*c].to_be_bytes());
        }
        section.extend([0xFF; 12]);
        let end = section.len() as u32;
        let mut out = Vec::new();
        out.extend([0x57, 0xE0, 0xE0, 0x57, 0x10, 0xC0, 0xC0, 0x10]);
        out.extend(0u32.to_be_bytes());
        out.extend(7u32.to_be_bytes());
        out.extend([4, 0, 1, 1]);
        out.extend(3u32.to_be_bytes());
        out.extend(2u32.to_be_bytes());
        out.extend(0u32.to_be_bytes());
        out.extend(0u32.to_be_bytes());
        out.extend(name_at[self.objects[0].1].to_be_bytes());
        let mut version = b"Havok-7.1.0-r1".to_vec();
        version.resize(16, 0);
        out.extend(version);
        out.extend([0xFF; 8]);
        assert_eq!(out.len(), 0x40);
        let classnames_start = 0x40 + 3 * 48;
        let data_start = classnames_start + names.len();
        for (tag, start, vals) in [
            ("__classnames__", classnames_start, [names.len() as u32; 6]),
            ("__types__", data_start, [0; 6]),
            (
                "__data__",
                data_start,
                [local_at, global_at, virtual_at, end, end, end],
            ),
        ] {
            let mut t = tag.as_bytes().to_vec();
            t.resize(19, 0);
            t.push(0xFF);
            out.extend(t);
            out.extend((start as u32).to_be_bytes());
            for v in vals {
                out.extend(v.to_be_bytes());
            }
        }
        out.extend(names);
        out.extend(section);
        out
    }
}

/// Root + a fixed rigid body (rotation 90 degrees about +Y, translation (10,20,30)) + its box shape.
fn scene() -> Vec<u8> {
    let mut b = Builder::new(0x230);
    b.object(0, "hkRootLevelContainer");
    b.object(0x40, "hkpRigidBody");
    b.object(0x200, "hkpBoxShape");
    b.global(0x40 + 0x10, 0x200);
    b.u32(0x40 + 0x14, 0xFFFF_FFFF);
    b.local(0x40 + 0x74, 0x1E0);
    b.data[0x1E0..0x1E5].copy_from_slice(b"Box1\0");
    b.data[0x40 + 0x8C] = 1;
    b.f32(0x40 + 0x90, 0.5);
    b.f32(0x40 + 0x94, 0.25);
    b.u32(0x40 + 0xA8, 0x0003_FFFF);
    b.data[0x40 + 0xE8] = 5;
    // columns: c0 = (0,0,-1), c1 = (0,1,0), c2 = (1,0,0)
    b.floats(0x40 + 0xF0, &[0.0, 0.0, -1.0]);
    b.floats(0x40 + 0x100, &[0.0, 1.0, 0.0]);
    b.floats(0x40 + 0x110, &[1.0, 0.0, 0.0]);
    b.floats(0x40 + 0x120, &[10.0, 20.0, 30.0]);
    b.f32(0x200 + 0x10, 0.05);
    b.floats(0x200 + 0x20, &[1.0, 2.0, 3.0]);
    b.build()
}

#[test]
fn rigid_body_and_box() {
    let bytes = scene();
    let p = Packfile::read(&bytes).unwrap();
    assert_eq!(p.objects().len(), 3);
    assert_eq!(p.class_at(0x40).unwrap(), "hkpRigidBody");
    assert_eq!(p.root(), 0);
    let c = physics::census(&p);
    assert_eq!(c.decoded.get("hkpRigidBody"), Some(&1));
    assert_eq!(c.decoded.get("hkpBoxShape"), Some(&1));
    assert_eq!(c.recognised.get("hkRootLevelContainer"), Some(&1));
    assert!(c.unknown.is_empty());
    assert_eq!(physics::rigid_bodies(&p), vec![0x40]);
    let body = RigidBody::read(&p, 0x40).unwrap();
    assert_eq!(body.name, "Box1");
    assert_eq!(body.shape, 0x200);
    assert_eq!(body.motion_type, MOTION_FIXED);
    assert_eq!((body.friction, body.restitution), (0.5, 0.25));
    assert_eq!(body.collision_filter_info, 0x0003_FFFF);
    // world = 1*c0 + 2*c1 + 3*c2 + t = (0,0,-1) + (0,2,0) + (3,0,0) + (10,20,30)
    assert_eq!(body.to_world([1.0, 2.0, 3.0]), [13.0, 22.0, 29.0]);
    assert_eq!(
        Shape::read(&p, 0x200).unwrap(),
        Shape::Box {
            radius: 0.05,
            half_extents: [1.0, 2.0, 3.0]
        }
    );
}

/// A shape-only fixture: an extended mesh with one subpart (the unit quad as two triangles).
fn mesh() -> Vec<u8> {
    let mut b = Builder::new(0x400);
    b.object(0, "hkRootLevelContainer");
    b.object(0x40, "hkpStorageExtendedMeshShape");
    b.object(0x200, "hkpStorageExtendedMeshShapeMeshSubpartStorage");
    let sub = 0x140; // one 96-byte subpart (trailing data of the shape)
    b.floats(0x40 + 0x90, &[0.5, 0.0, 0.5]);
    b.floats(0x40 + 0xA0, &[0.5, 0.0, 0.5]);
    b.u32(0x40 + 0xB0, 12);
    b.array(0x40 + 0xB4, Some(sub), 1);
    b.array(0x40 + 0xC0, None, 0);
    b.array(0x40 + 0xCC, Some(0x1C0), 2);
    b.array(0x40 + 0xF0, Some(0x1D0), 1);
    b.global(0x1D0, 0x200);
    b.u16(0x1C0, 0x1111);
    b.u16(0x1C2, 0x2222);
    b.u32(sub + 0x08, 0x0002_0001);
    b.u32(sub + 0x14, 2);
    b.u32(sub + 0x1C, 16);
    b.u32(sub + 0x20, 4);
    b.u32(sub + 0x44, 16);
    b.u32(sub + 0x48, 0x0300_0000);
    let s = 0x200;
    b.array(s + 0x08, Some(0x280), 4);
    b.array(s + 0x14, None, 0);
    b.array(s + 0x20, None, 0);
    b.array(s + 0x2C, Some(0x2C0), 8);
    b.array(s + 0x38, None, 0);
    b.array(s + 0x44, Some(0x2F0), 1);
    b.array(s + 0x50, None, 0);
    b.array(s + 0x5C, Some(0x320), 2);
    for (i, v) in [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0]]
        .iter()
        .enumerate()
    {
        b.floats(0x280 + 16 * i, v);
    }
    for (i, w) in [0u32, 1, 2, 0, 0, 2, 3, 0].iter().enumerate() {
        b.u32(0x2C0 + 4 * i, *w);
    }
    b.u32(0x2F0, 0x58); // surface (0x58 >> 3) & 0x3F = 11
    b.u32(0x2F4, 0x3F80);
    b.u16(0x320, 0);
    b.u16(0x322, 0);
    b.build()
}

#[test]
fn extended_mesh() {
    let bytes = mesh();
    let p = Packfile::read(&bytes).unwrap();
    let m = ExtendedMesh::read(&p, 0x40).unwrap();
    assert_eq!(m.subparts.len(), 1);
    let s = &m.subparts[0];
    assert_eq!(
        s.vertices,
        vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0]]
    );
    assert_eq!(s.triangles, vec![[0, 1, 2], [0, 2, 3]]);
    assert_eq!(s.material_indices, vec![0, 0]);
    assert_eq!(s.materials.len(), 1);
    assert_eq!(s.materials[0].a, 0x58);
    assert_eq!((s.materials[0].a >> 3) & 0x3F, 11);
    assert_eq!(m.welding, vec![0x1111, 0x2222]);
    assert_eq!(m.triangle_count(), 2);
    assert_eq!(m.aabb_center, [0.5, 0.0, 0.5]);
}

fn convex() -> Vec<u8> {
    let mut b = Builder::new(0x200);
    b.object(0, "hkRootLevelContainer");
    b.object(0x40, "hkpConvexVerticesShape");
    b.f32(0x40 + 0x10, 0.05);
    b.floats(0x40 + 0x20, &[2.0, 2.0, 2.0]);
    b.floats(0x40 + 0x30, &[2.0, 12.0, 22.0]);
    b.array(0x40 + 0x40, Some(0x100), 2);
    b.u32(0x40 + 0x4C, 5);
    b.array(0x40 + 0x54, Some(0x180), 1);
    // vertex j = (j, 10 + j, 20 + j); group g holds x[4], y[4], z[4] for vertices 4g..4g+4 (missing lanes 0)
    for g in 0..2 {
        for axis in 0..3 {
            for lane in 0..4 {
                let j = 4 * g + lane;
                let v = if j < 5 { (j + 10 * axis) as f32 } else { 0.0 };
                b.f32(0x100 + 48 * g + 16 * axis + 4 * lane, v);
            }
        }
    }
    b.floats(0x180, &[0.0, 1.0, 0.0, -14.0]);
    b.build()
}

#[test]
fn convex_vertices_four_vectors() {
    let bytes = convex();
    let p = Packfile::read(&bytes).unwrap();
    let c = ConvexVertices::read(&p, 0x40).unwrap();
    let want: Vec<[f32; 3]> = (0..5)
        .map(|j| [j as f32, 10.0 + j as f32, 20.0 + j as f32])
        .collect();
    assert_eq!(c.vertices, want);
    assert_eq!(c.planes, vec![[0.0, 1.0, 0.0, -14.0]]);
    assert_eq!(c.radius, 0.05);
}

fn refuse(bytes: &[u8]) -> String {
    Packfile::read(bytes).unwrap_err().to_string()
}

#[test]
fn refusals_name_the_offset() {
    let good = scene();
    assert!(Packfile::read(&good).is_ok());
    let patch = |at: usize, v: &[u8]| {
        let mut b = good.clone();
        b[at..at + v.len()].copy_from_slice(v);
        b
    };
    assert!(refuse(&patch(0, &[0])).contains("magic"));
    let e = refuse(&patch(0x0C, &[0, 0, 0, 8]));
    assert!(e.contains("version 8") && e.contains("0xc"), "{e}");
    assert!(refuse(&patch(0x10, &[8])).contains("pointer size 8"));
    assert!(refuse(&patch(0x11, &[1])).contains("little endian"));
    assert!(refuse(&patch(0x12, &[0])).contains("reuse padding 0"));
    assert!(refuse(&patch(0x28, b"Havok-5.5.0-r1\0")).contains("contents version"));
    // the class-name section is the first at 0x40 + 144; break its separator byte
    assert!(refuse(&patch(0x40 + 3 * 48 + 4, &[7])).contains("0xFF padding"));
    // section start inside the header table
    assert!(refuse(&patch(0x40 + 20, &[0, 0, 0, 0x10])).contains("outside the file"));
    // truncated file
    assert!(refuse(&good[..good.len() - 40]).contains("outside the file"));
}

#[test]
fn fixup_refusals() {
    let base = scene();
    // locate the data section's fixup tables from the header (data section header at 0x40 + 96)
    let hdr = 0x40 + 96 + 20;
    let rd = |o: usize| u32::from_be_bytes(base[o..o + 4].try_into().unwrap()) as usize;
    let (start, local, global, virt) = (rd(hdr), rd(hdr + 4), rd(hdr + 8), rd(hdr + 12));
    let patch = |at: usize, v: u32| {
        let mut b = base.clone();
        b[at..at + 4].copy_from_slice(&v.to_be_bytes());
        b
    };
    // a local fixup whose target is beyond the data
    let e = refuse(&patch(start + local + 4, 0x10_0000));
    assert!(e.contains("outside the data"), "{e}");
    // a local fixup whose source slot is beyond the data
    assert!(refuse(&patch(start + local, 0x10_0000)).contains("outside the data"));
    // a global fixup into another section
    assert!(refuse(&patch(start + global + 4, 1)).contains("global fixup into section 1"));
    // a virtual fixup naming something that is not a class name
    let e = refuse(&patch(start + virt + 8, 3));
    assert!(e.contains("not a name"), "{e}");
    // a virtual fixup naming the wrong section
    assert!(refuse(&patch(start + virt + 4, 1)).contains("not the class names"));
    // two virtual fixups for one object
    let e = refuse(&patch(start + virt + 12, 0));
    assert!(e.contains("two virtual fixups") || e.contains("root"), "{e}");
}

#[test]
fn pointer_and_array_checks() {
    // a pointer slot with no fixup must hold zero
    let mut b = Builder::new(0x230);
    b.object(0, "hkRootLevelContainer");
    b.object(0x40, "hkpRigidBody");
    b.object(0x200, "hkpBoxShape");
    b.u32(0x40 + 0x10, 0x200); // no fixup
    let bytes = b.build();
    let p = Packfile::read(&bytes).unwrap();
    let e = RigidBody::read(&p, 0x40).unwrap_err().to_string();
    assert!(e.contains("no fixup"), "{e}");
    // an array longer than its capacity
    let mut b = Builder::new(0x400);
    b.object(0, "hkRootLevelContainer");
    b.object(0x40, "hkpStorageExtendedMeshShape");
    b.u32(0x40 + 0xB0, 12);
    b.local(0x40 + 0xB4, 0x140);
    b.u32(0x40 + 0xB4 + 4, 5);
    b.u32(0x40 + 0xB4 + 8, 0x8000_0001);
    let bytes = b.build();
    let p = Packfile::read(&bytes).unwrap();
    let e = ExtendedMesh::read(&p, 0x40).unwrap_err().to_string();
    assert!(e.contains("exceeds its capacity"), "{e}");
}

#[test]
fn mesh_refusals() {
    let bytes = mesh();
    // the data section ends the file: its start is the file length minus the header's `end`
    let rd = |o: usize| u32::from_be_bytes(bytes[o..o + 4].try_into().unwrap()) as usize;
    let data_start = bytes.len() - rd(0x40 + 96 + 20 + 24);
    let patch = |at: usize, v: u32| {
        let mut x = bytes.clone();
        x[data_start + at..data_start + at + 4].copy_from_slice(&v.to_be_bytes());
        x
    };
    let e = {
        let x = patch(0x2C0, 9);
        let p = Packfile::read(&x).unwrap();
        ExtendedMesh::read(&p, 0x40).unwrap_err().to_string()
    };
    assert!(e.contains("outside the vertices"), "{e}");
    let e = {
        let x = patch(0x2C0 + 12, 1);
        let p = Packfile::read(&x).unwrap();
        ExtendedMesh::read(&p, 0x40).unwrap_err().to_string()
    };
    assert!(e.contains("fourth index word"), "{e}");
    let e = {
        let x = patch(0x2F4, 0x3F81);
        let p = Packfile::read(&x).unwrap();
        ExtendedMesh::read(&p, 0x40).unwrap_err().to_string()
    };
    assert!(e.contains("0x3F80"), "{e}");
    let e = {
        let x = patch(0x320, 0x0001_0000); // material index 1 of 1
        let p = Packfile::read(&x).unwrap();
        ExtendedMesh::read(&p, 0x40).unwrap_err().to_string()
    };
    assert!(e.contains("outside the materials"), "{e}");
}

#[test]
fn non_finite_floats_are_errors() {
    let bytes = {
        let mut b = Builder::new(0x230);
        b.object(0, "hkRootLevelContainer");
        b.object(0x40, "hkpRigidBody");
        b.object(0x200, "hkpBoxShape");
        b.f32(0x200 + 0x10, f32::NAN);
        b.build()
    };
    let p = Packfile::read(&bytes).unwrap();
    assert!(
        Shape::read(&p, 0x200)
            .unwrap_err()
            .to_string()
            .contains("non-finite")
    );
}
