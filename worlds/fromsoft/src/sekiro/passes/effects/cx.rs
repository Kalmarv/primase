//! The context of one effect's lowering: the game, what resolves the files an action names, the external values the
//! properties use so far, and the accounting of what was lost or approximated. The helpers here make the core values and
//! terms the appearances are built of.

use super::value::{self, Inputs, Notes};
use fromsoft_formats::Game;
use fromsoft_formats::fxr::Property;
use fromsoft_formats::fxr::resources::TextureKind;
use primase_core::Entity;
use primase_core::particles::{Argument, Channels, Map, Term, Value};

/// What resolves the numbers an effect names to the entities that hold them in the world being lowered into. Each answer is
/// `None` where the file is not there; the lowering counts that and carries on without it.
pub trait Resolver {
    /// The texture entity of kind `kind` and number `id`.
    fn texture(&mut self, kind: TextureKind, id: i32) -> Option<Entity>;
    /// The prototype entity of model particle `id`.
    fn model(&mut self, id: i32) -> Option<Entity>;
    /// The clip entity of model animation `id`.
    fn animation(&mut self, id: i32) -> Option<Entity>;
    /// The system entity of the effect `id` (an effect an `SFXReference` spawns); made on first use.
    fn effect(&mut self, id: i32) -> Option<Entity>;
}

/// A resolver that finds nothing: what an effect lowers to with none of its files (a test of the structure, a count of what
/// it would need).
#[derive(Clone, Copy, Debug, Default)]
pub struct NoFiles;

impl Resolver for NoFiles {
    fn texture(&mut self, _: TextureKind, _: i32) -> Option<Entity> {
        None
    }
    fn model(&mut self, _: i32) -> Option<Entity> {
        None
    }
    fn animation(&mut self, _: i32) -> Option<Entity> {
        None
    }
    fn effect(&mut self, _: i32) -> Option<Entity> {
        None
    }
}

/// The state of one lowering.
pub struct Cx<'r> {
    pub game: Game,
    pub resolver: &'r mut dyn Resolver,
    pub inputs: Inputs,
    pub notes: Notes,
}

impl<'r> Cx<'r> {
    pub fn new(game: Game, resolver: &'r mut dyn Resolver) -> Self {
        Cx {
            game,
            resolver,
            inputs: Inputs::default(),
            notes: Notes::default(),
        }
    }

    /// A property as a value of `argument`.
    pub fn val(&mut self, p: &Property, argument: Argument) -> Value {
        value::value(p, argument, &mut self.inputs, &mut self.notes)
    }

    /// A property read at the particle's age.
    pub fn age(&mut self, p: &Property) -> Value {
        self.val(p, Argument::Age)
    }

    /// A property read at the emitter's active time.
    pub fn active(&mut self, p: &Property) -> Value {
        self.val(p, Argument::ActiveTime)
    }

    /// A property read at the time the particle was emitted.
    pub fn emitted(&mut self, p: &Property) -> Value {
        self.val(p, Argument::EmissionTime)
    }

    /// A property read at no argument (the files' "constant 0").
    pub fn fixed(&mut self, p: &Property) -> Value {
        self.val(p, Argument::Constant)
    }

    /// A constant scalar.
    pub fn scalar(&self, v: f32) -> Value {
        Value::constant(&[v])
    }

    /// A texture a property (a constant, stored as a float) names; `None` where it names none or the file is absent.
    pub fn map_prop(&mut self, kind: TextureKind, p: &Property) -> Option<Map> {
        self.map_id(kind, value::id(p))
    }

    /// A texture a field word names; a word of 0 or less names none.
    pub fn map_word(&mut self, kind: TextureKind, word: i32) -> Option<Map> {
        self.map_id(kind, (word > 0).then_some(word))
    }

    fn map_id(&mut self, kind: TextureKind, id: Option<i32>) -> Option<Map> {
        let id = id?;
        match self.resolver.texture(kind, id) {
            Some(texture) => Some(Map { texture }),
            None => {
                self.notes.lose("a texture that is not in the effect binders");
                None
            }
        }
    }

    /// One term of a colour: `p` as a factor of `channels` at `argument`, clamped to 0 to 1 first when `clamp`.
    pub fn term(&mut self, p: &Property, argument: Argument, clamp: bool, channels: Channels) -> Term {
        Term {
            value: self.val(p, argument),
            clamp,
            channels,
        }
    }

    /// The colour factors of an appearance: its three colour multipliers (the first two clamped to 0 to 1, read at the age and
    /// the emission time, the third unrestricted), the brightness multiplier of the red, green and blue and the alpha multiplier
    /// (both read at the emitter's active time). A factor that is the constant 1 is left out.
    pub fn tint(
        &mut self,
        c1: &Property,
        c2: &Property,
        c3: &Property,
        rgb: &Property,
        alpha: &Property,
    ) -> Vec<Term> {
        let terms = vec![
            self.term(c1, Argument::Age, true, Channels::Rgba),
            self.term(c2, Argument::EmissionTime, true, Channels::Rgba),
            self.term(c3, Argument::Age, false, Channels::Rgba),
            self.term(rgb, Argument::ActiveTime, false, Channels::Rgb),
            self.term(alpha, Argument::ActiveTime, false, Channels::Alpha),
        ];
        without_identities(terms)
    }

    /// A rotation about the three axes in degrees (read at no argument): the file's right-handed rotations about x and y turn
    /// the other way in core's mirrored frame, so those two are negated; z is unchanged.
    pub fn rotation(&mut self, r: [&Property; 3]) -> [Value; 3] {
        let x = self.fixed(r[0]);
        let y = self.fixed(r[1]);
        let z = self.fixed(r[2]);
        [value::negated(x), value::negated(y), z]
    }

    /// A spin about the three axes in degrees per second: each the speed times its multiplier, both read at the age, signed as
    /// [`Cx::rotation`] is.
    pub fn spin(&mut self, s: [(&Property, &Property); 3]) -> [Value; 3] {
        let mut out = Vec::with_capacity(3);
        for (i, (speed, multiplier)) in s.into_iter().enumerate() {
            let (a, b) = (self.age(speed), self.age(multiplier));
            let v = value::product(a, b, &mut self.notes);
            out.push(if i < 2 { value::negated(v) } else { v });
        }
        let z = out.pop().unwrap_or_else(|| Value::constant(&[0.0]));
        let y = out.pop().unwrap_or_else(|| Value::constant(&[0.0]));
        let x = out.pop().unwrap_or_else(|| Value::constant(&[0.0]));
        [x, y, z]
    }
}

/// `terms` without the factors that are exactly 1 in every channel they carry.
pub fn without_identities(mut terms: Vec<Term>) -> Vec<Term> {
    terms.retain(|t| {
        let width = usize::from(t.value.curve.width.clamp(1, 4));
        !(t.value.is_fixed() && t.value.curve.value_at(0.0).iter().take(width).all(|&x| x == 1.0))
    });
    terms
}
