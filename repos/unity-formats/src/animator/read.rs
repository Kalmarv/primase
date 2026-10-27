//! Reading an Avatar, an AnimatorController and an Animator into the typed data of [`crate::animator`]. Every field is
//! required: a missing or mistyped one is an error naming it and the object (nothing defaults to zero).

use crate::animator::{
    AvatarAxes, AvatarHuman, AvatarSkeleton, BlendNode, Condition, HumanBone, HumanDescription, Layer,
    Parameter, SELECTOR_BASE, SelectorState, SelectorTransition, SkeletonBone, State, StateMachine,
    Transition, UnityAnimatorController, UnityAvatar, Xform,
};
use crate::player::{ObjRef, Player};
use crate::value::{Array, Value};
use crate::{Result, invalid};

/// A field reader over one object: errors name `what` and the field.
struct Fields<'a> {
    what: &'a str,
}

impl Fields<'_> {
    fn get<'v>(&self, v: &'v Value, k: &str) -> Result<&'v Value> {
        v.get(k).ok_or_else(|| invalid(format!("{}: no {k}", self.what)))
    }

    /// The `data` of an offset-pointer wrapper (`{"data": {...}}`).
    fn data<'v>(&self, v: &'v Value) -> Result<&'v Value> {
        self.get(v, "data")
    }

    fn int(&self, v: &Value, k: &str) -> Result<i64> {
        self.get(v, k)?
            .as_i64()
            .ok_or_else(|| invalid(format!("{}: {k} is not an integer", self.what)))
    }

    fn uint(&self, v: &Value, k: &str) -> Result<u32> {
        u32::try_from(self.int(v, k)?)
            .map_err(|_| invalid(format!("{}: {k} is not a 32-bit unsigned integer", self.what)))
    }

    fn float(&self, v: &Value, k: &str) -> Result<f32> {
        self.get(v, k)?
            .as_f32()
            .ok_or_else(|| invalid(format!("{}: {k} is not a float", self.what)))
    }

    fn flag(&self, v: &Value, k: &str) -> Result<bool> {
        match self.get(v, k)? {
            Value::Bool(b) => Ok(*b),
            x => x
                .as_i64()
                .map(|i| i != 0)
                .ok_or_else(|| invalid(format!("{}: {k} is not a flag", self.what))),
        }
    }

    fn string(&self, v: &Value, k: &str) -> Result<String> {
        self.get(v, k)?
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| invalid(format!("{}: {k} is not a string", self.what)))
    }

    fn vec3(&self, v: &Value, k: &str) -> Result<[f32; 3]> {
        let x = self.get(v, k)?;
        Ok([self.float(x, "x")?, self.float(x, "y")?, self.float(x, "z")?])
    }

    fn vec2(&self, x: &Value) -> Result<[f32; 2]> {
        Ok([self.float(x, "x")?, self.float(x, "y")?])
    }

    /// A stored rotation, kept as stored; one of no length or not finite is no rotation and refused.
    fn quat(&self, v: &Value, k: &str) -> Result<[f32; 4]> {
        let x = self.get(v, k)?;
        let q = [
            self.float(x, "x")?,
            self.float(x, "y")?,
            self.float(x, "z")?,
            self.float(x, "w")?,
        ];
        let length = q.iter().map(|&x| f64::from(x) * f64::from(x)).sum::<f64>().sqrt();
        if !(length > 0.0 && length.is_finite()) {
            return Err(invalid(format!("{}: {k} {q:?} is no rotation", self.what)));
        }
        Ok(q)
    }

    /// The elements of a list of structs; an empty list of any element type is empty.
    fn list<'v>(&self, v: &'v Value, k: &str) -> Result<&'v [Value]> {
        match self.get(v, k)? {
            Value::Array(Array::Values(items)) => Ok(items),
            Value::Array(a) if a.is_empty() => Ok(&[]),
            _ => Err(invalid(format!("{}: {k} is not a list", self.what))),
        }
    }

    /// A list of integers of any width.
    fn ints(&self, v: &Value, k: &str) -> Result<Vec<i64>> {
        let bad = || invalid(format!("{}: {k} is not a list of integers", self.what));
        Ok(match self.get(v, k)? {
            Value::Array(Array::I8(x)) => x.iter().map(|&i| i64::from(i)).collect(),
            Value::Array(Array::I16(x)) => x.iter().map(|&i| i64::from(i)).collect(),
            Value::Array(Array::I32(x)) => x.iter().map(|&i| i64::from(i)).collect(),
            Value::Array(Array::I64(x)) => x.clone(),
            Value::Array(Array::U8(x)) => x.iter().map(|&i| i64::from(i)).collect(),
            Value::Array(Array::U16(x)) => x.iter().map(|&i| i64::from(i)).collect(),
            Value::Array(Array::U32(x)) => x.iter().map(|&i| i64::from(i)).collect(),
            Value::Array(Array::U64(x)) => x
                .iter()
                .map(|&i| i64::try_from(i).map_err(|_| bad()))
                .collect::<Result<_>>()?,
            Value::Array(Array::Values(x)) => x
                .iter()
                .map(|i| i.as_i64().ok_or_else(bad))
                .collect::<Result<_>>()?,
            Value::Array(a) if a.is_empty() => Vec::new(),
            _ => return Err(bad()),
        })
    }

    fn uints(&self, v: &Value, k: &str) -> Result<Vec<u32>> {
        self.ints(v, k)?
            .into_iter()
            .map(|i| {
                u32::try_from(i).map_err(|_| {
                    invalid(format!(
                        "{}: {k} holds {i}, not a 32-bit unsigned integer",
                        self.what
                    ))
                })
            })
            .collect()
    }

    fn floats(&self, v: &Value, k: &str) -> Result<Vec<f32>> {
        let bad = || invalid(format!("{}: {k} is not a list of floats", self.what));
        Ok(match self.get(v, k)? {
            Value::Array(Array::F32(x)) => x.clone(),
            Value::Array(Array::Values(x)) => x
                .iter()
                .map(|i| i.as_f32().ok_or_else(bad))
                .collect::<Result<_>>()?,
            Value::Array(a) if a.is_empty() => Vec::new(),
            _ => return Err(bad()),
        })
    }

    fn bools(&self, v: &Value, k: &str) -> Result<Vec<bool>> {
        let bad = || invalid(format!("{}: {k} is not a list of flags", self.what));
        Ok(match self.get(v, k)? {
            Value::Array(Array::Bool(x)) => x.clone(),
            Value::Array(Array::U8(x)) => x.iter().map(|&b| b != 0).collect(),
            Value::Array(Array::Values(x)) => x
                .iter()
                .map(|i| match i {
                    Value::Bool(b) => Ok(*b),
                    other => other.as_i64().map(|n| n != 0).ok_or_else(bad),
                })
                .collect::<Result<_>>()?,
            Value::Array(a) if a.is_empty() => Vec::new(),
            _ => return Err(bad()),
        })
    }

    /// A list of `(hash, text)` pairs (`m_TOS`).
    fn tos(&self, v: &Value, k: &str) -> Result<Vec<(u32, String)>> {
        self.list(v, k)?
            .iter()
            .map(|p| match p {
                Value::Pair(pair) => {
                    let hash = pair
                        .0
                        .as_i64()
                        .and_then(|h| u32::try_from(h).ok())
                        .ok_or_else(|| {
                            invalid(format!("{}: {k} holds a hash that is not 32 bits", self.what))
                        })?;
                    let text = pair.1.as_str().ok_or_else(|| {
                        invalid(format!("{}: {k} holds a name that is not a string", self.what))
                    })?;
                    Ok((hash, text.to_owned()))
                }
                _ => Err(invalid(format!(
                    "{}: {k} holds something that is not a pair",
                    self.what
                ))),
            })
            .collect()
    }

    fn xform(&self, v: &Value) -> Result<Xform> {
        Ok(Xform {
            t: self.vec3(v, "t")?,
            q: self.quat(v, "q")?,
            s: self.vec3(v, "s")?,
        })
    }

    /// `{"data": {"m_X": [xform, ...]}}`.
    fn pose(&self, v: &Value) -> Result<Vec<Xform>> {
        let d = self.data(v)?;
        self.list(d, "m_X")?.iter().map(|x| self.xform(x)).collect()
    }

    /// `{"data": {"m_Node", "m_ID", "m_AxesArray"}}`.
    fn skeleton(&self, v: &Value) -> Result<AvatarSkeleton> {
        let d = self.data(v)?;
        let nodes = self
            .list(d, "m_Node")?
            .iter()
            .map(|n| Ok((self.int(n, "m_ParentId")?, self.int(n, "m_AxesId")?)))
            .collect::<Result<Vec<_>>>()?;
        let ids = self.uints(d, "m_ID")?;
        if ids.len() != nodes.len() {
            return Err(invalid(format!(
                "{}: {} node ids for {} nodes",
                self.what,
                ids.len(),
                nodes.len()
            )));
        }
        let axes = self
            .list(d, "m_AxesArray")?
            .iter()
            .map(|a| {
                let limit = self.get(a, "m_Limit")?;
                Ok(AvatarAxes {
                    pre_q: self.quat(a, "m_PreQ")?,
                    post_q: self.quat(a, "m_PostQ")?,
                    sgn: self.vec3(a, "m_Sgn")?,
                    min: self.vec3(limit, "m_Min")?,
                    max: self.vec3(limit, "m_Max")?,
                    length: self.float(a, "m_Length")?,
                    kind: self.int(a, "m_Type")?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        for (k, &(parent, axes_id)) in nodes.iter().enumerate() {
            let in_range = |i: i64, n: usize| i == -1 || usize::try_from(i).is_ok_and(|i| i < n);
            if !in_range(parent, nodes.len()) || !in_range(axes_id, axes.len()) {
                return Err(invalid(format!(
                    "{}: node {k} names parent {parent} and axes {axes_id} of {} nodes and {} axes",
                    self.what,
                    nodes.len(),
                    axes.len()
                )));
            }
        }
        Ok(AvatarSkeleton { nodes, ids, axes })
    }
}

fn human(f: &Fields, v: &Value) -> Result<Option<AvatarHuman>> {
    let d = f.data(v)?;
    let skeleton = f.skeleton(f.get(d, "m_Skeleton")?)?;
    // a generic avatar's human data is empty
    if skeleton.nodes.is_empty() {
        return Ok(None);
    }
    let hand = |k: &str| f.ints(f.data(f.get(d, k)?)?, "m_HandBoneIndex");
    let pose = f.pose(f.get(d, "m_SkeletonPose")?)?;
    if pose.len() != skeleton.nodes.len() {
        return Err(invalid(format!(
            "{}: {} human poses for {} human nodes",
            f.what,
            pose.len(),
            skeleton.nodes.len()
        )));
    }
    Ok(Some(AvatarHuman {
        root: f.xform(f.get(d, "m_RootX")?)?,
        skeleton,
        pose,
        left_hand: hand("m_LeftHand")?,
        right_hand: hand("m_RightHand")?,
        bone_index: f.ints(d, "m_HumanBoneIndex")?,
        bone_mass: f.floats(d, "m_HumanBoneMass")?,
        scale: f.float(d, "m_Scale")?,
        arm_twist: f.float(d, "m_ArmTwist")?,
        fore_arm_twist: f.float(d, "m_ForeArmTwist")?,
        upper_leg_twist: f.float(d, "m_UpperLegTwist")?,
        leg_twist: f.float(d, "m_LegTwist")?,
        arm_stretch: f.float(d, "m_ArmStretch")?,
        leg_stretch: f.float(d, "m_LegStretch")?,
        feet_spacing: f.float(d, "m_FeetSpacing")?,
        has_left_hand: f.flag(d, "m_HasLeftHand")?,
        has_right_hand: f.flag(d, "m_HasRightHand")?,
        has_translation_dof: f.flag(d, "m_HasTDoF")?,
    }))
}

fn description(f: &Fields, v: &Value) -> Result<HumanDescription> {
    let human = f
        .list(v, "m_Human")?
        .iter()
        .map(|b| {
            let limit = f.get(b, "m_Limit")?;
            Ok(HumanBone {
                bone_name: f.string(b, "m_BoneName")?,
                human_name: f.string(b, "m_HumanName")?,
                min: f.vec3(limit, "m_Min")?,
                max: f.vec3(limit, "m_Max")?,
                value: f.vec3(limit, "m_Value")?,
                length: f.float(limit, "m_Length")?,
                modified: f.flag(limit, "m_Modified")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let skeleton = f
        .list(v, "m_Skeleton")?
        .iter()
        .map(|b| {
            Ok(SkeletonBone {
                name: f.string(b, "m_Name")?,
                parent: f.string(b, "m_ParentName")?,
                position: f.vec3(b, "m_Position")?,
                rotation: f.quat(b, "m_Rotation")?,
                scale: f.vec3(b, "m_Scale")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(HumanDescription {
        human,
        skeleton,
        arm_twist: f.float(v, "m_ArmTwist")?,
        fore_arm_twist: f.float(v, "m_ForeArmTwist")?,
        upper_leg_twist: f.float(v, "m_UpperLegTwist")?,
        leg_twist: f.float(v, "m_LegTwist")?,
        arm_stretch: f.float(v, "m_ArmStretch")?,
        leg_stretch: f.float(v, "m_LegStretch")?,
        feet_spacing: f.float(v, "m_FeetSpacing")?,
        global_scale: f.float(v, "m_GlobalScale")?,
        root_motion_bone_name: f.string(v, "m_RootMotionBoneName")?,
        has_translation_dof: f.flag(v, "m_HasTranslationDoF")?,
        has_extra_root: f.flag(v, "m_HasExtraRoot")?,
        skeleton_has_parents: f.flag(v, "m_SkeletonHasParents")?,
    })
}

impl UnityAvatar {
    /// The Avatar object `r`.
    pub fn read(player: &Player, r: &ObjRef) -> Result<UnityAvatar> {
        let v = player.decode(r)?;
        let name = v.get("m_Name").and_then(Value::as_str).unwrap_or("").to_owned();
        let what = format!("Avatar {}/{} {name:?}", r.file, r.path_id);
        let f = Fields { what: &what };
        let a = f.get(&v, "m_Avatar")?;
        let skeleton = f.skeleton(f.get(a, "m_AvatarSkeleton")?)?;
        let skeleton_pose = f.pose(f.get(a, "m_AvatarSkeletonPose")?)?;
        let default_pose = f.pose(f.get(a, "m_DefaultPose")?)?;
        for (pose, which) in [(&skeleton_pose, "skeleton pose"), (&default_pose, "default pose")] {
            if pose.len() != skeleton.nodes.len() {
                return Err(invalid(format!(
                    "{what}: {} {which} transforms for {} nodes",
                    pose.len(),
                    skeleton.nodes.len()
                )));
            }
        }
        let root_motion_skeleton = f.skeleton(f.get(a, "m_RootMotionSkeleton")?)?;
        let root_motion_pose = f.pose(f.get(a, "m_RootMotionSkeletonPose")?)?;
        // the root motion skeleton's pose is stored with its nodes or not at all (SkateBIRD's seagull and little bird have
        // two nodes and no pose)
        if !root_motion_pose.is_empty() && root_motion_pose.len() != root_motion_skeleton.nodes.len() {
            return Err(invalid(format!(
                "{what}: {} root motion poses for {} root motion nodes",
                root_motion_pose.len(),
                root_motion_skeleton.nodes.len()
            )));
        }
        Ok(UnityAvatar {
            avatar: r.clone(),
            name,
            skeleton,
            skeleton_pose,
            default_pose,
            skeleton_name_ids: f.uints(a, "m_SkeletonNameIDArray")?,
            human: human(&f, f.get(a, "m_Human")?)?,
            human_skeleton_index: f.ints(a, "m_HumanSkeletonIndexArray")?,
            human_skeleton_reverse: f.ints(a, "m_HumanSkeletonReverseIndexArray")?,
            root_motion_bone_index: f.int(a, "m_RootMotionBoneIndex")?,
            root_motion_bone: f.xform(f.get(a, "m_RootMotionBoneX")?)?,
            root_motion_skeleton,
            root_motion_pose,
            root_motion_skeleton_index: f.ints(a, "m_RootMotionSkeletonIndexArray")?,
            tos: f.tos(&v, "m_TOS")?,
            description: description(&f, f.get(&v, "m_HumanDescription")?)?,
        })
    }
}

fn conditions(f: &Fields, v: &Value, k: &str) -> Result<Vec<Condition>> {
    f.list(v, k)?
        .iter()
        .map(|c| {
            let c = f.data(c)?;
            Ok(Condition {
                mode: f.int(c, "m_ConditionMode")?,
                parameter: f.uint(c, "m_EventID")?,
                threshold: f.float(c, "m_EventThreshold")?,
                exit_time: f.float(c, "m_ExitTime")?,
            })
        })
        .collect()
}

fn transition(f: &Fields, t: &Value) -> Result<Transition> {
    let t = f.data(t)?;
    Ok(Transition {
        conditions: conditions(f, t, "m_ConditionConstantArray")?,
        destination: f.int(t, "m_DestinationState")?,
        full_path_id: f.uint(t, "m_FullPathID")?,
        id: f.uint(t, "m_ID")?,
        user_id: f.uint(t, "m_UserID")?,
        duration: f.float(t, "m_TransitionDuration")?,
        offset: f.float(t, "m_TransitionOffset")?,
        exit_time: f.float(t, "m_ExitTime")?,
        has_exit_time: f.flag(t, "m_HasExitTime")?,
        has_fixed_duration: f.flag(t, "m_HasFixedDuration")?,
        interruption_source: f.int(t, "m_InterruptionSource")?,
        ordered_interruption: f.flag(t, "m_OrderedInterruption")?,
        can_transition_to_self: f.flag(t, "m_CanTransitionToSelf")?,
    })
}

fn blend_node(f: &Fields, n: &Value) -> Result<BlendNode> {
    let n = f.data(n)?;
    let one = f.data(f.get(n, "m_Blend1dData")?)?;
    let two = f.data(f.get(n, "m_Blend2dData")?)?;
    let direct = f.data(f.get(n, "m_BlendDirectData")?)?;
    Ok(BlendNode {
        blend_type: f.int(n, "m_BlendType")?,
        event_id: f.uint(n, "m_BlendEventID")?,
        event_y_id: f.uint(n, "m_BlendEventYID")?,
        children: f.uints(n, "m_ChildIndices")?,
        thresholds: f.floats(one, "m_ChildThresholdArray")?,
        positions: f
            .list(two, "m_ChildPositionArray")?
            .iter()
            .map(|p| f.vec2(p))
            .collect::<Result<_>>()?,
        magnitudes: f.floats(two, "m_ChildMagnitudeArray")?,
        pair_vectors: f
            .list(two, "m_ChildPairVectorArray")?
            .iter()
            .map(|p| f.vec2(p))
            .collect::<Result<_>>()?,
        pair_avg_mag_inv: f.floats(two, "m_ChildPairAvgMagInvArray")?,
        neighbors: f
            .list(two, "m_ChildNeighborListArray")?
            .iter()
            .map(|l| f.uints(l, "m_NeighborArray"))
            .collect::<Result<_>>()?,
        direct_events: f.uints(direct, "m_ChildBlendEventIDArray")?,
        normalized_blend_values: f.flag(direct, "m_NormalizedBlendValues")?,
        clip: f.uint(n, "m_ClipID")?,
        duration: f.float(n, "m_Duration")?,
        cycle_offset: f.float(n, "m_CycleOffset")?,
        mirror: f.flag(n, "m_Mirror")?,
    })
}

fn state(f: &Fields, s: &Value) -> Result<State> {
    let s = f.data(s)?;
    Ok(State {
        transitions: f
            .list(s, "m_TransitionConstantArray")?
            .iter()
            .map(|t| transition(f, t))
            .collect::<Result<_>>()?,
        blend_tree_index: f.ints(s, "m_BlendTreeConstantIndexArray")?,
        blend_trees: f
            .list(s, "m_BlendTreeConstantArray")?
            .iter()
            .map(|tree| {
                let tree = f.data(tree)?;
                f.list(tree, "m_NodeArray")?
                    .iter()
                    .map(|n| blend_node(f, n))
                    .collect::<Result<Vec<_>>>()
            })
            .collect::<Result<_>>()?,
        name_id: f.uint(s, "m_NameID")?,
        path_id: f.uint(s, "m_PathID")?,
        full_path_id: f.uint(s, "m_FullPathID")?,
        tag_id: f.uint(s, "m_TagID")?,
        speed_param: f.uint(s, "m_SpeedParamID")?,
        mirror_param: f.uint(s, "m_MirrorParamID")?,
        cycle_offset_param: f.uint(s, "m_CycleOffsetParamID")?,
        time_param: f.uint(s, "m_TimeParamID")?,
        speed: f.float(s, "m_Speed")?,
        cycle_offset: f.float(s, "m_CycleOffset")?,
        ik_on_feet: f.flag(s, "m_IKOnFeet")?,
        write_default_values: f.flag(s, "m_WriteDefaultValues")?,
        looping: f.flag(s, "m_Loop")?,
        mirror: f.flag(s, "m_Mirror")?,
    })
}

fn state_machine(f: &Fields, m: &Value) -> Result<StateMachine> {
    let m = f.data(m)?;
    Ok(StateMachine {
        states: f
            .list(m, "m_StateConstantArray")?
            .iter()
            .map(|s| state(f, s))
            .collect::<Result<_>>()?,
        any_state_transitions: f
            .list(m, "m_AnyStateTransitionConstantArray")?
            .iter()
            .map(|t| transition(f, t))
            .collect::<Result<_>>()?,
        selectors: f
            .list(m, "m_SelectorStateConstantArray")?
            .iter()
            .map(|s| {
                let s = f.data(s)?;
                Ok(SelectorState {
                    transitions: f
                        .list(s, "m_TransitionConstantArray")?
                        .iter()
                        .map(|t| {
                            let t = f.data(t)?;
                            Ok(SelectorTransition {
                                destination: f.uint(t, "m_Destination")?,
                                conditions: conditions(f, t, "m_ConditionConstantArray")?,
                            })
                        })
                        .collect::<Result<_>>()?,
                    full_path_id: f.uint(s, "m_FullPathID")?,
                    is_entry: f.flag(s, "m_IsEntry")?,
                })
            })
            .collect::<Result<_>>()?,
        default_state: f.int(m, "m_DefaultState")?,
        synchronized_layer_count: f.int(m, "m_SynchronizedLayerCount")?,
    })
}

fn layer(f: &Fields, l: &Value) -> Result<Layer> {
    let l = f.data(l)?;
    let mask = f.get(l, "m_BodyMask")?;
    Ok(Layer {
        state_machine: f.int(l, "m_StateMachineIndex")?,
        synchronized_layer: f.int(l, "m_StateMachineSynchronizedLayerIndex")?,
        body_mask: [
            f.uint(mask, "word0")?,
            f.uint(mask, "word1")?,
            f.uint(mask, "word2")?,
        ],
        skeleton_mask: f
            .list(f.data(f.get(l, "m_SkeletonMask")?)?, "m_Data")?
            .iter()
            .map(|e| Ok((f.uint(e, "m_PathHash")?, f.float(e, "m_Weight")?)))
            .collect::<Result<_>>()?,
        binding: f.uint(l, "m_Binding")?,
        blending_mode: f.int(l, "(int&)m_LayerBlendingMode")?,
        default_weight: f.float(l, "m_DefaultWeight")?,
        ik_pass: f.flag(l, "m_IKPass")?,
        synced_layer_affects_timing: f.flag(l, "m_SyncedLayerAffectsTiming")?,
    })
}

impl UnityAnimatorController {
    /// The AnimatorController object `r`.
    pub fn read(player: &Player, r: &ObjRef) -> Result<UnityAnimatorController> {
        let v = player.decode(r)?;
        let name = v.get("m_Name").and_then(Value::as_str).unwrap_or("").to_owned();
        let what = format!("AnimatorController {}/{} {name:?}", r.file, r.path_id);
        let f = Fields { what: &what };
        let c = f.get(&v, "m_Controller")?;
        let values = f.data(f.get(c, "m_Values")?)?;
        let defaults = f.data(f.get(c, "m_DefaultValues")?)?;
        let pointers = |k: &str| -> Result<Vec<ObjRef>> {
            let mut out = Vec::new();
            for p in f.list(&v, k)? {
                if let Some(o) = player.resolve(&r.file, p)? {
                    out.push(o);
                }
            }
            Ok(out)
        };
        let controller = UnityAnimatorController {
            controller: r.clone(),
            name,
            layers: f
                .list(c, "m_LayerArray")?
                .iter()
                .map(|l| layer(&f, l))
                .collect::<Result<_>>()?,
            state_machines: f
                .list(c, "m_StateMachineArray")?
                .iter()
                .map(|m| state_machine(&f, m))
                .collect::<Result<_>>()?,
            parameters: f
                .list(values, "m_ValueArray")?
                .iter()
                .map(|p| {
                    Ok(Parameter {
                        id: f.uint(p, "m_ID")?,
                        kind: f.int(p, "m_Type")?,
                        index: f.int(p, "m_Index")?,
                    })
                })
                .collect::<Result<_>>()?,
            default_floats: f.floats(defaults, "m_FloatValues")?,
            default_ints: f.ints(defaults, "m_IntValues")?,
            default_bools: f.bools(defaults, "m_BoolValues")?,
            clips: f
                .list(&v, "m_AnimationClips")?
                .iter()
                .map(|p| player.resolve(&r.file, p))
                .collect::<Result<_>>()?,
            behaviours: pointers("m_StateMachineBehaviours")?,
            tos: f.tos(&v, "m_TOS")?,
        };
        controller.check(&what)?;
        Ok(controller)
    }

    /// The references inside the controller against what they index: layers' state machines, transitions' destinations,
    /// blend trees' children and clips, parameters' defaults.
    fn check(&self, what: &str) -> Result<()> {
        let bad = |why: String| invalid(format!("{what}: {why}"));
        for (k, l) in self.layers.iter().enumerate() {
            if usize::try_from(l.state_machine).map_or(true, |i| i >= self.state_machines.len()) {
                return Err(bad(format!(
                    "layer {k} runs state machine {} of {}",
                    l.state_machine,
                    self.state_machines.len()
                )));
            }
        }
        for (m, machine) in self.state_machines.iter().enumerate() {
            let states = machine.states.len() as i64;
            let selectors = machine.selectors.len() as i64;
            // a destination is a state (below SELECTOR_BASE) or SELECTOR_BASE plus a selector node (an exit, or an entry
            // of another machine)
            let reaches =
                |d: i64| (0..states).contains(&d) || (SELECTOR_BASE..SELECTOR_BASE + selectors).contains(&d);
            for t in machine
                .states
                .iter()
                .flat_map(|s| &s.transitions)
                .chain(&machine.any_state_transitions)
            {
                if !reaches(t.destination) {
                    return Err(bad(format!(
                        "state machine {m}: a transition to {} of {states} states and {selectors} selectors",
                        t.destination
                    )));
                }
            }
            for t in machine.selectors.iter().flat_map(|s| &s.transitions) {
                if !reaches(i64::from(t.destination)) {
                    return Err(bad(format!(
                        "state machine {m}: a selector transition to {} of {states} states and {selectors} selectors",
                        t.destination
                    )));
                }
            }
            for s in &machine.states {
                for tree in &s.blend_trees {
                    for n in tree {
                        if n.children.iter().any(|&c| c as usize >= tree.len()) {
                            return Err(bad(format!(
                                "state machine {m}: a blend tree child beyond its {} nodes",
                                tree.len()
                            )));
                        }
                        if n.children.is_empty() && n.clip != u32::MAX && n.clip as usize >= self.clips.len()
                        {
                            return Err(bad(format!(
                                "state machine {m}: a blend tree leaf plays clip {} of {}",
                                n.clip,
                                self.clips.len()
                            )));
                        }
                    }
                }
            }
        }
        for p in &self.parameters {
            let count = match p.kind {
                1 => self.default_floats.len(),
                3 => self.default_ints.len(),
                4 | 9 => self.default_bools.len(),
                k => return Err(bad(format!("a parameter of kind {k}"))),
            };
            if usize::try_from(p.index).map_or(true, |i| i >= count) {
                return Err(bad(format!(
                    "a parameter of kind {} at {} of {count} defaults",
                    p.kind, p.index
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn s(fields: &[(&str, Value)]) -> Value {
        Value::Struct(fields.iter().map(|(k, v)| (Arc::from(*k), v.clone())).collect())
    }

    fn list(items: Vec<Value>) -> Value {
        Value::Array(Array::Values(items))
    }

    fn vec3(x: f32, y: f32, z: f32) -> Value {
        s(&[
            ("x", Value::Float(x)),
            ("y", Value::Float(y)),
            ("z", Value::Float(z)),
        ])
    }

    fn quat(x: f32, y: f32, z: f32, w: f32) -> Value {
        s(&[
            ("x", Value::Float(x)),
            ("y", Value::Float(y)),
            ("z", Value::Float(z)),
            ("w", Value::Float(w)),
        ])
    }

    /// A skeleton of two nodes, the second under the first with the one axes entry.
    #[test]
    fn a_skeleton_reads_with_its_nodes_ids_and_axes() {
        let axes = s(&[
            ("m_PreQ", quat(0.0, 0.0, 0.0, 1.0)),
            ("m_PostQ", quat(0.5, 0.5, 0.5, 0.5)),
            ("m_Sgn", vec3(1.0, -1.0, 1.0)),
            (
                "m_Limit",
                s(&[("m_Min", vec3(-1.0, -2.0, -3.0)), ("m_Max", vec3(1.0, 2.0, 3.0))]),
            ),
            ("m_Length", Value::Float(0.25)),
            ("m_Type", Value::Int(1)),
        ]);
        let node = |p: i64, a: i64| s(&[("m_ParentId", Value::Int(p)), ("m_AxesId", Value::Int(a))]);
        let wrap = |nodes: Vec<Value>, ids: Vec<u32>, axes: Vec<Value>| {
            s(&[(
                "data",
                s(&[
                    ("m_Node", list(nodes)),
                    ("m_ID", Value::Array(Array::U32(ids))),
                    ("m_AxesArray", list(axes)),
                ]),
            )])
        };
        let f = Fields { what: "t" };
        let sk = f
            .skeleton(&wrap(
                vec![node(-1, -1), node(0, 0)],
                vec![7, 9],
                vec![axes.clone()],
            ))
            .unwrap();
        assert_eq!(sk.nodes, vec![(-1, -1), (0, 0)]);
        assert_eq!(sk.ids, vec![7, 9]);
        assert_eq!(sk.axes[0].sgn, [1.0, -1.0, 1.0]);
        assert_eq!(
            (sk.axes[0].min, sk.axes[0].max),
            ([-1.0, -2.0, -3.0], [1.0, 2.0, 3.0])
        );
        assert_eq!((sk.axes[0].length, sk.axes[0].kind), (0.25, 1));
        // an id missing, a parent and an axes index out of range
        let e = |w: Value| f.skeleton(&w).unwrap_err().to_string();
        assert!(
            e(wrap(vec![node(-1, -1), node(0, 0)], vec![7], vec![axes.clone()]))
                .contains("1 node ids for 2 nodes")
        );
        assert!(e(wrap(vec![node(5, -1)], vec![7], vec![])).contains("names parent 5"));
        assert!(e(wrap(vec![node(-1, 0)], vec![7], vec![])).contains("axes 0"));
    }

    /// Fields are required and typed: a missing one and one of another type are errors naming the field and the object.
    #[test]
    fn fields_are_required_and_typed() {
        let f = Fields {
            what: "Avatar a/1 \"x\"",
        };
        let v = s(&[("m_Scale", Value::Float(2.0)), ("m_Name", Value::Int(3))]);
        assert_eq!(f.float(&v, "m_Scale").unwrap(), 2.0);
        let missing = f.float(&v, "m_Other").unwrap_err().to_string();
        assert!(
            missing.contains("Avatar a/1") && missing.contains("no m_Other"),
            "{missing}"
        );
        let mistyped = f.string(&v, "m_Name").unwrap_err().to_string();
        assert!(mistyped.contains("m_Name is not a string"), "{mistyped}");
        assert!(f.uint(&s(&[("k", Value::Int(-1))]), "k").is_err());
        assert!(
            f.uints(&s(&[("k", Value::Array(Array::I32(vec![1, -2])))]), "k")
                .is_err()
        );
        assert_eq!(
            f.ints(&s(&[("k", Value::Array(Array::I32(vec![1, -2])))]), "k")
                .unwrap(),
            vec![1, -2]
        );
        assert!(f.ints(&s(&[("k", list(vec![]))]), "k").unwrap().is_empty());
    }

    /// The name table is pairs of a hash and a name.
    #[test]
    fn name_tables_read_as_pairs() {
        let f = Fields { what: "t" };
        let pair =
            |h: u32, n: &str| Value::Pair(Box::new((Value::UInt(u64::from(h)), Value::String(n.into()))));
        let v = s(&[("m_TOS", list(vec![pair(1, "a"), pair(2, "b/c")]))]);
        assert_eq!(
            f.tos(&v, "m_TOS").unwrap(),
            vec![(1, "a".to_owned()), (2, "b/c".to_owned())]
        );
        let bad = s(&[("m_TOS", list(vec![Value::Int(3)]))]);
        assert!(
            f.tos(&bad, "m_TOS")
                .unwrap_err()
                .to_string()
                .contains("not a pair")
        );
    }

    /// A controller's internal references are checked: a layer's state machine, a transition's destination, a blend
    /// tree's child and clip, a parameter's default.
    #[test]
    fn a_controller_is_checked_against_what_its_indices_name() {
        let leaf = |clip: u32| BlendNode {
            blend_type: 0,
            event_id: u32::MAX,
            event_y_id: u32::MAX,
            children: vec![],
            thresholds: vec![],
            positions: vec![],
            magnitudes: vec![],
            pair_vectors: vec![],
            pair_avg_mag_inv: vec![],
            neighbors: vec![],
            direct_events: vec![],
            normalized_blend_values: false,
            clip,
            duration: 1.0,
            cycle_offset: 0.0,
            mirror: false,
        };
        let state = |clip: u32, to: i64| State {
            transitions: vec![Transition {
                conditions: vec![],
                destination: to,
                full_path_id: 0,
                id: 0,
                user_id: 0,
                duration: 0.1,
                offset: 0.0,
                exit_time: 0.0,
                has_exit_time: false,
                has_fixed_duration: true,
                interruption_source: 0,
                ordered_interruption: true,
                can_transition_to_self: true,
            }],
            blend_tree_index: vec![0],
            blend_trees: vec![vec![leaf(clip)]],
            name_id: 1,
            path_id: 1,
            full_path_id: 1,
            tag_id: 0,
            speed_param: 0,
            mirror_param: 0,
            cycle_offset_param: 0,
            time_param: 0,
            speed: 1.0,
            cycle_offset: 0.0,
            ik_on_feet: false,
            write_default_values: true,
            looping: true,
            mirror: false,
        };
        let controller = |clip: u32, to: i64, machine: i64, param_index: i64| UnityAnimatorController {
            controller: ObjRef {
                file: "f".into(),
                path_id: 1,
            },
            name: "c".into(),
            layers: vec![Layer {
                state_machine: machine,
                synchronized_layer: 0,
                body_mask: [u32::MAX; 3],
                skeleton_mask: vec![],
                binding: 5,
                blending_mode: 0,
                default_weight: 1.0,
                ik_pass: false,
                synced_layer_affects_timing: false,
            }],
            state_machines: vec![StateMachine {
                states: vec![state(clip, to)],
                any_state_transitions: vec![],
                selectors: vec![],
                default_state: 0,
                synchronized_layer_count: 1,
            }],
            parameters: vec![Parameter {
                id: 9,
                kind: 1,
                index: param_index,
            }],
            default_floats: vec![0.0],
            default_ints: vec![],
            default_bools: vec![],
            // the second slot is a null pointer: it still counts as a clip index
            clips: vec![
                Some(ObjRef {
                    file: "f".into(),
                    path_id: 2,
                }),
                None,
            ],
            behaviours: vec![],
            tos: vec![(5, "Base Layer".into())],
        };
        assert!(controller(0, 0, 0, 0).check("c").is_ok());
        assert!(controller(1, 0, 0, 0).check("c").is_ok());
        assert!(controller(0, 0, 0, 0).name_of(5) == Some("Base Layer"));
        let e = |c: UnityAnimatorController| c.check("c").unwrap_err().to_string();
        assert!(e(controller(3, 0, 0, 0)).contains("plays clip 3 of 2"));
        assert!(e(controller(0, 4, 0, 0)).contains("a transition to 4"));
        // a selector destination is SELECTOR_BASE plus its index: with none, 30000 is no destination
        assert!(e(controller(0, 30000, 0, 0)).contains("a transition to 30000"));
        assert!(e(controller(0, 0, 2, 0)).contains("runs state machine 2 of 1"));
        assert!(e(controller(0, 0, 0, 3)).contains("at 3 of 1 defaults"));
    }
}
