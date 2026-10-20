//! `omit-skatebird-mission-props{roles=R+R...}`: every entity under a script of the stated roles
//! ([`SkatebirdUnder`], from [`crate::read::read_scripted`]) is left out: objects the game shows only for a while or
//! for the player, which a static level drawing every one of them stacks into the scene: a mission's collectibles
//! (shown while their mission runs, every random candidate stacked), its giver birds with their dressing, the unlocks
//! (shown until collected), the player's own board, and what hangs under a scene camera (the HUD's mission arrow). Counted per group (the second part of the script's GameObject
//! path under a `Missions - <level>` or `Unlocks` root, else its root) and role. The roles are the program's choice, stated (no default); a
//! script stored disabled counts like an enabled one. World state that missions switch (`MissionToggled`) is not a
//! role and stays as the scene stores it. Run it right after the read, before passes that make entities from these.

use std::collections::BTreeSet;

use primase_core::World;
use primase_core::pass::{Options, Pass, PassLog};

use crate::components::{SkatebirdRole, SkatebirdScript, SkatebirdUnder};

/// See the module documentation. `roles`: at least one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OmitSkatebirdMissionProps {
    pub roles: BTreeSet<SkatebirdRole>,
}

impl OmitSkatebirdMissionProps {
    /// From its options: `roles`, required, role words joined by `+` ([`SkatebirdRole::word`]).
    pub fn from_options(o: &Options) -> Result<OmitSkatebirdMissionProps, String> {
        if let Some(k) = o.keys().find(|k| k.as_str() != "roles") {
            return Err(format!("unknown option {k}"));
        }
        let text = o
            .get("roles")
            .ok_or("roles is required: role words joined by + (collectible, enable_one_at_random, mission_giver, birb_decorator, unlocking_collectible, player_board, camera)")?;
        let roles = (text.split('+'))
            .map(|w| SkatebirdRole::of_word(w).ok_or_else(|| format!("roles: unknown role {w}")))
            .collect::<Result<BTreeSet<_>, _>>()?;
        Ok(OmitSkatebirdMissionProps { roles })
    }
}

/// The group a script belongs to: the part after a `Missions - <level>` or `Unlocks` root, else the root.
fn mission(s: &SkatebirdScript) -> &str {
    let mut parts = s.path.split('/');
    let root = parts.next().unwrap_or("");
    match parts.next() {
        Some(m) if root.starts_with("Missions") || root == "Unlocks" => m,
        _ => root,
    }
}

impl Pass for OmitSkatebirdMissionProps {
    fn name(&self) -> &'static str {
        "omit-skatebird-mission-props"
    }
    fn options(&self) -> Options {
        let words: Vec<&str> = self.roles.iter().map(|r| r.word()).collect();
        Options::from([("roles".to_owned(), words.join("+"))])
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        if self.roles.is_empty() {
            return Err("omit-skatebird-mission-props needs at least one role".into());
        }
        let gone: Vec<(primase_core::Entity, String)> = w
            .iter::<SkatebirdUnder>()
            .filter_map(|(e, u)| {
                // the outermost script of a stated role names what is left out
                let s = u.0.iter().find(|s| self.roles.contains(&s.role))?;
                Some((e, format!("left out under {} ({})", mission(s), s.role.word())))
            })
            .collect();
        for (e, what) in gone {
            w.despawn(e);
            log.count(&what, 1);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::{Name, Transform};
    use primase_unity::components::UnityObject;

    fn script(role: SkatebirdRole, path: &str) -> SkatebirdScript {
        SkatebirdScript {
            role,
            behaviour: UnityObject {
                file: "level1".into(),
                path_id: 1,
            },
            enabled: true,
            path: path.into(),
        }
    }

    /// Entities under a collectible or a giver go, counted per mission; one under a random pick alone (not a stated
    /// role) and one under nothing stay. The options read back.
    #[test]
    fn props_under_the_stated_roles_are_left_out_per_mission() {
        let mut w = World::new();
        let place = |w: &mut World, name: &str, under: Vec<SkatebirdScript>| {
            let e = w.spawn();
            w.insert(e, Transform::IDENTITY);
            w.insert(e, Name(name.into()));
            if !under.is_empty() {
                w.insert(e, SkatebirdUnder(under));
            }
            e
        };
        let snack = place(
            &mut w,
            "pretzel",
            vec![
                script(
                    SkatebirdRole::Collectible,
                    "Missions - Bedroom 1/Munch The Leftovers/Snack 3",
                ),
                script(
                    SkatebirdRole::EnableOneAtRandom,
                    "Missions - Bedroom 1/Munch The Leftovers/Snack 3/Pick",
                ),
            ],
        );
        let bird = place(
            &mut w,
            "bird",
            vec![script(
                SkatebirdRole::MissionGiver,
                "Missions - Bedroom 1/Crucibird/Giver",
            )],
        );
        let pick = place(
            &mut w,
            "candle",
            vec![script(SkatebirdRole::EnableOneAtRandom, "Level Elements/Candles")],
        );
        let plain = place(&mut w, "bed", Vec::new());
        let o = Options::from([("roles".to_owned(), "collectible+mission_giver".to_owned())]);
        let pass = OmitSkatebirdMissionProps::from_options(&o).unwrap();
        assert_eq!(pass.options(), o);
        let mut log = PassLog::default();
        pass.run(&mut w, &mut log).unwrap();
        assert!(!w.is_alive(snack) && !w.is_alive(bird));
        assert!(w.is_alive(pick) && w.is_alive(plain));
        assert_eq!(
            log.counts.get("left out under Munch The Leftovers (collectible)"),
            Some(&1),
            "{:?}",
            log.counts
        );
        assert_eq!(
            log.counts.get("left out under Crucibird (mission_giver)"),
            Some(&1),
            "{:?}",
            log.counts
        );
        assert!(OmitSkatebirdMissionProps::from_options(&Options::new()).is_err());
        let unknown = Options::from([("roles".to_owned(), "npc".to_owned())]);
        assert!(OmitSkatebirdMissionProps::from_options(&unknown).is_err());
    }
}
