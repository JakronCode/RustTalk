//! Companions: little pets that live above the user panel while you are
//! connected to a server and travel with you from server to server.
//!
//! This module is the UI-free part: the companions you have (name, coat,
//! collar), how many you may have ([`Store::slots`]), the progress saved on
//! disk (`companions.toml` next to the config: time together, visited
//! servers, pets, snacks, hunger) and the mood logic that picks an
//! [`Emotion`]. Drawing and the adoption flow live in `app::companion_ui`.
//!
//! Everything is local: nothing about a companion is sent to a server.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Every kind of companion there is. New ones: a variant here, an entry in
/// [`Kind::info`] and a paint function in `app::companion_ui`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Dog,
}

pub struct Info {
    /// Species shown in lists ("Dog").
    pub species: &'static str,
    /// Coats a new one is rolled from (the painter uses the same order).
    pub coats: &'static [&'static str],
}

impl Kind {
    pub fn info(self) -> Info {
        match self {
            Kind::Dog => Info { species: "Dog", coats: &["Red & white", "Shiba red", "Black & white", "Black & tan", "White", "Silver"] },
        }
    }
}

/// Names offered for a new companion (the dice picks another).
pub const NAMES: [&str; 16] =
    ["Mochi", "Kiko", "Yuki", "Hachi", "Momo", "Sora", "Kuma", "Taro", "Hana", "Riku", "Nori", "Pochi", "Azuki", "Kinako", "Daifuku", "Sakura"];

/// Collar colors to pick from (any other color works too).
pub const COLLARS: [[u8; 3]; 10] = [
    [0xE5, 0x48, 0x4D],
    [0xF0, 0x6A, 0xA8],
    [0xF5, 0x9E, 0x42],
    [0xF2, 0xC9, 0x4C],
    [0x4C, 0xC3, 0x8A],
    [0x2E, 0xB8, 0xB8],
    [0x4C, 0x8D, 0xF6],
    [0x9B, 0x6C, 0xF2],
    [0x2B, 0x2B, 0x33],
    [0xF2, 0xF2, 0xF2],
];

/// The first companion is free; one more for every this many hours together.
pub const HOURS_PER_SLOT: f64 = 10.0;
/// Hours of time together until a full companion is hungry again.
const HOURS_TO_HUNGRY: f64 = 4.0;
/// From this hunger on the companion shows that it is hungry.
pub const HUNGRY_AT: f32 = 0.7;
/// Below this hunger a snack is refused ("I'm full").
pub const FULL_BELOW: f32 = 0.15;
/// Longest name.
pub const MAX_NAME: usize = 20;

/// Time spent on one server.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Visit {
    /// Server name at the last visit.
    pub name: String,
    pub seconds: f64,
    /// Unix seconds.
    pub first: u64,
    pub last: u64,
}

/// One companion and everything remembered about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Companion {
    /// Stable id (the config remembers which one comes along).
    pub id: u32,
    pub kind: Kind,
    pub name: String,
    /// Index into [`Info::coats`] (rolled at adoption).
    #[serde(default)]
    pub coat: u8,
    /// Small individual traits (markings, fluff, tint), see [`genes`].
    #[serde(default)]
    pub genes: u32,
    pub collar: [u8; 3],
    /// Unix seconds of the first meeting.
    #[serde(default)]
    pub met: u64,
    /// Seconds shown while connected to a server.
    #[serde(default)]
    pub seconds_together: f64,
    #[serde(default)]
    pub pets: u64,
    #[serde(default)]
    pub snacks: u64,
    /// 0 = just ate, 1 = very hungry. Only grows while you are together.
    #[serde(default)]
    pub hunger: f32,
    /// Servers it has been on, by normalized address.
    #[serde(default)]
    pub visits: BTreeMap<String, Visit>,
    /// 0 = miserable, 1 = as happy as can be. Petting and snacks raise it;
    /// it sinks slowly while you are together (faster when hungry).
    #[serde(default = "default_happiness")]
    pub happiness: f32,
    /// Bond with you, in points (see [`BOND_LEVELS`]). Only ever grows: with
    /// time together (faster when happy) and with snacks.
    #[serde(default)]
    pub bond: f32,
}

fn default_happiness() -> f32 {
    0.7
}

/// Hours together until a happy companion that gets no attention is miserable.
const HOURS_TO_UNHAPPY: f64 = 10.0;
/// Below this happiness it looks sad.
pub const SAD_BELOW: f32 = 0.25;
/// Bond levels: points needed and what the level is called.
pub const BOND_LEVELS: [(f32, &str); 6] =
    [(0.0, "Just met"), (1.0, "Acquaintance"), (5.0, "Buddy"), (15.0, "Good friend"), (40.0, "Best friend"), (100.0, "Inseparable")];

impl Companion {
    /// A fresh companion (id 0, never met): for previews and [`Store::adopt`].
    pub fn new(kind: Kind, name: &str, coat: u8, genes: u32, collar: [u8; 3]) -> Self {
        Self {
            id: 0,
            kind,
            name: clean_name(name).unwrap_or_else(|| NAMES[0].to_string()),
            coat,
            genes,
            collar,
            met: 0,
            seconds_together: 0.0,
            pets: 0,
            snacks: 0,
            hunger: 0.0,
            visits: BTreeMap::new(),
            happiness: default_happiness(),
            bond: 0.0,
        }
    }

    fn cheer_up(&mut self, by: f32) {
        self.happiness = (self.happiness + by).clamp(0.0, 1.0);
    }

    /// One completed pet.
    pub fn petted(&mut self) {
        self.pets += 1;
        self.cheer_up(0.04);
    }

    /// Ate a snack.
    pub fn fed(&mut self) {
        self.snacks += 1;
        self.hunger = 0.0;
        self.cheer_up(0.2);
        self.bond += 0.25;
    }

    /// Made grumpy (booped too often or woken up).
    pub fn annoyed(&mut self) {
        self.cheer_up(-0.08);
    }

    /// Bond level: index into [`BOND_LEVELS`], its name and the progress
    /// towards the next level (1.0 on the last one).
    pub fn bond_level(&self) -> (usize, &'static str, f32) {
        let i = BOND_LEVELS.iter().rposition(|(at, _)| self.bond >= *at).unwrap_or(0);
        let progress = match BOND_LEVELS.get(i + 1) {
            Some((next, _)) => (self.bond - BOND_LEVELS[i].0) / (next - BOND_LEVELS[i].0),
            None => 1.0,
        };
        (i, BOND_LEVELS[i].1, progress.clamp(0.0, 1.0))
    }

    pub fn coat_name(&self) -> &'static str {
        let coats = self.kind.info().coats;
        coats[self.coat as usize % coats.len()]
    }
}

/// All companions (the `companions.toml` file).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Store {
    /// What this user's companions are rolled from (0 = not rolled yet).
    /// Random for now; meant to become the fingerprint of the user id.
    #[serde(default)]
    pub seed: u32,
    #[serde(default)]
    pub companions: Vec<Companion>,
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// A random number (for names, coats and collars).
pub fn random() -> u32 {
    let mut b = [0u8; 4];
    if getrandom::fill(&mut b).is_err() {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(7);
        return n.wrapping_mul(2_654_435_761);
    }
    u32::from_le_bytes(b)
}

/// What companion number `slot` looks like for the user with this `seed`
/// ([`Store::seed`]): everyone gets their own, and the same seed always
/// gives the same one.
pub fn genes(seed: &str, slot: usize) -> [u8; 8] {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(format!("rusttalk-companion|{seed}|{slot}").as_bytes());
    hash[..8].try_into().expect("8 bytes")
}

/// Picks one of `items` at random.
pub fn pick<T: Copy>(items: &[T]) -> T {
    items[random() as usize % items.len()]
}

/// Cleans up a typed name (trimmed, not too long; empty = None).
pub fn clean_name(name: &str) -> Option<String> {
    let n: String = name.trim().chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
    let n = n.trim().to_string();
    (!n.is_empty()).then_some(n)
}

impl Store {
    /// Next to the config file.
    pub fn path() -> PathBuf {
        crate::install::config_path().with_file_name("companions.toml")
    }

    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save_to(&self, path: &Path) {
        match toml::to_string_pretty(self) {
            Ok(text) => {
                if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                    let _ = std::fs::create_dir_all(dir);
                }
                if let Err(e) = std::fs::write(path, text) {
                    tracing::warn!("Could not save companions: {e}");
                }
            }
            Err(e) => tracing::warn!("Could not serialize companions: {e}"),
        }
    }

    /// The seed this user's companions come from (rolled on first use).
    pub fn seed(&mut self) -> String {
        while self.seed == 0 {
            self.seed = random();
        }
        format!("{:08x}", self.seed)
    }

    /// Hours with all companions together.
    pub fn total_hours(&self) -> f64 {
        self.companions.iter().map(|c| c.seconds_together).sum::<f64>() / 3600.0
    }

    /// How many companions you may have right now.
    pub fn slots(&self) -> usize {
        1 + (self.total_hours() / HOURS_PER_SLOT) as usize
    }

    /// Room for a new companion?
    pub fn can_adopt(&self) -> bool {
        self.companions.len() < self.slots()
    }

    /// Hours together until the next slot opens.
    pub fn hours_to_next_slot(&self) -> f64 {
        let needed = self.companions.len().max(1) as f64 * HOURS_PER_SLOT;
        (needed - self.total_hours()).max(0.0)
    }

    pub fn get(&self, id: u32) -> Option<&Companion> {
        self.companions.iter().find(|c| c.id == id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Companion> {
        self.companions.iter_mut().find(|c| c.id == id)
    }

    /// Adds a new companion and returns its id.
    pub fn adopt(&mut self, kind: Kind, name: &str, coat: u8, genes: u32, collar: [u8; 3]) -> u32 {
        let id = self.companions.iter().map(|c| c.id).max().map_or(1, |m| m + 1);
        self.companions.push(Companion { id, met: now_unix(), hunger: 0.3, ..Companion::new(kind, name, coat, genes, collar) });
        id
    }

    /// Counts `secs` together on a server. Returns true on the first visit
    /// there (the companion has something to say about it).
    pub fn spend(&mut self, id: u32, server_key: &str, server_name: &str, secs: f64) -> bool {
        let now = now_unix();
        let Some(c) = self.get_mut(id) else { return false };
        c.seconds_together += secs;
        let hours = secs / 3600.0;
        // Bond first: the happier it is, the faster it grows.
        c.bond += (hours * (0.5 + c.happiness as f64)) as f32;
        let sulk = if c.hunger >= HUNGRY_AT { 2.0 } else { 1.0 };
        c.happiness = (c.happiness - (hours * sulk / HOURS_TO_UNHAPPY) as f32).max(0.0);
        c.hunger = (c.hunger + (hours / HOURS_TO_HUNGRY) as f32).min(1.0);
        let mut new = false;
        let v = c.visits.entry(server_key.to_string()).or_insert_with(|| {
            new = true;
            Visit { first: now, ..Default::default() }
        });
        v.name = server_name.to_string();
        v.seconds += secs;
        v.last = now;
        new
    }
}

/// What the companion feels right now (picked by [`Mood::emotion`]). New
/// emotions: a variant here, a rule there and a face in `app::companion_ui`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emotion {
    /// Nothing special going on.
    Content,
    /// Just booped or fed.
    Happy,
    /// Being petted right now.
    Petted,
    /// Chewing a snack.
    Eating,
    /// Wants a snack.
    Hungry,
    /// No attention for a while.
    Sleepy,
    /// No attention for a long while.
    Asleep,
    /// Booped too often, or woken up by a boop.
    Angry,
    /// Offered a snack while full.
    Full,
    /// Not happy: too little attention for too long.
    Sad,
}

impl Emotion {
    pub fn label(self) -> &'static str {
        match self {
            Emotion::Content => "content",
            Emotion::Happy => "happy",
            Emotion::Petted => "loving it",
            Emotion::Eating => "munching",
            Emotion::Hungry => "hungry",
            Emotion::Sleepy => "sleepy",
            Emotion::Asleep => "asleep",
            Emotion::Angry => "grumpy",
            Emotion::Full => "full",
            Emotion::Sad => "sad",
        }
    }
}

/// No attention for this long -> sleepy, then asleep (seconds).
pub const SLEEPY_AFTER: f64 = 8.0 * 60.0;
pub const ASLEEP_AFTER: f64 = 20.0 * 60.0;
/// This many boops within [`BOOP_WINDOW`] seconds make it grumpy.
const BOOPS_TO_ANGRY: usize = 5;
const BOOP_WINDOW: f64 = 4.0;
/// Seconds of petting that count as one pet (and calm a grumpy companion).
pub const PET_SECS: f64 = 1.2;

/// The companion's short-term mood (not saved). All times are seconds on
/// one clock (the UI's frame time).
#[derive(Debug, Clone)]
pub struct Mood {
    last_attention: f64,
    happy_until: f64,
    eating_until: f64,
    angry_until: f64,
    full_until: f64,
    petting_until: f64,
    pet_progress: f64,
    boops: VecDeque<f64>,
}

impl Mood {
    pub fn new(now: f64) -> Self {
        Self {
            last_attention: now,
            happy_until: 0.0,
            eating_until: 0.0,
            angry_until: 0.0,
            full_until: 0.0,
            petting_until: 0.0,
            pet_progress: 0.0,
            boops: VecDeque::new(),
        }
    }

    pub fn emotion(&self, now: f64, hunger: f32) -> Emotion {
        let idle = now - self.last_attention;
        if now < self.eating_until {
            Emotion::Eating
        } else if now < self.angry_until {
            Emotion::Angry
        } else if now < self.full_until {
            Emotion::Full
        } else if now < self.petting_until {
            Emotion::Petted
        } else if now < self.happy_until {
            Emotion::Happy
        } else if idle >= ASLEEP_AFTER {
            Emotion::Asleep
        } else if hunger >= HUNGRY_AT {
            Emotion::Hungry
        } else if idle >= SLEEPY_AFTER {
            Emotion::Sleepy
        } else {
            Emotion::Content
        }
    }

    /// The emotion to show for `c`: like [`Mood::emotion`], but an unhappy
    /// companion is sad when nothing else is going on.
    pub fn feel(&self, now: f64, c: &Companion) -> Emotion {
        match self.emotion(now, c.hunger) {
            Emotion::Content if c.happiness < SAD_BELOW => Emotion::Sad,
            e => e,
        }
    }

    /// Something nice happened that isn't an interaction (e.g. a new server).
    pub fn cheer(&mut self, now: f64, secs: f64) {
        self.last_attention = now;
        self.happy_until = self.happy_until.max(now + secs);
    }

    /// A click on the companion.
    pub fn boop(&mut self, now: f64, hunger: f32) {
        let was_asleep = self.emotion(now, hunger) == Emotion::Asleep;
        self.last_attention = now;
        self.boops.push_back(now);
        while self.boops.front().is_some_and(|t| now - t > BOOP_WINDOW) {
            self.boops.pop_front();
        }
        if was_asleep || self.boops.len() >= BOOPS_TO_ANGRY {
            self.angry_until = now + if was_asleep { 4.0 } else { 6.0 };
            self.boops.clear();
        } else if now >= self.angry_until {
            self.happy_until = now + 1.6;
        }
    }

    /// Petting for `dt` seconds. Returns true when that completed a pet.
    /// Petting calms a grumpy companion.
    pub fn pet(&mut self, now: f64, dt: f64) -> bool {
        self.last_attention = now;
        self.petting_until = now + 0.6;
        self.pet_progress += dt.clamp(0.0, 0.25);
        if self.pet_progress >= PET_SECS {
            self.pet_progress = 0.0;
            self.angry_until = 0.0;
            self.happy_until = now + 2.0;
            return true;
        }
        false
    }

    /// Offers a snack. Returns true if it was eaten (the caller resets hunger).
    pub fn feed(&mut self, now: f64, hunger: f32) -> bool {
        self.last_attention = now;
        if hunger < FULL_BELOW {
            self.full_until = now + 2.5;
            return false;
        }
        self.angry_until = 0.0;
        self.eating_until = now + 2.4;
        self.happy_until = now + 5.0;
        true
    }

    /// Seconds since the companion started eating (for the chewing animation).
    pub fn eating_progress(&self, now: f64) -> Option<f32> {
        (now < self.eating_until).then(|| (1.0 - (self.eating_until - now) / 2.4) as f32)
    }
}

/// "3 h 12 min", "12 min", "40 s".
pub fn format_duration(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    match (s / 3600, (s % 3600) / 60) {
        (0, 0) => format!("{s} s"),
        (0, m) => format!("{m} min"),
        (h, m) => format!("{h} h {m} min"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_dog() -> (Store, u32) {
        let mut s = Store::default();
        let id = s.adopt(Kind::Dog, "  Mochi  ", 2, 7, [1, 2, 3]);
        (s, id)
    }

    #[test]
    fn store_roundtrip() {
        let (mut s, id) = store_with_dog();
        assert!(s.spend(id, "10.0.0.5:7331", "Home", 90.0), "first visit");
        assert!(!s.spend(id, "10.0.0.5:7331", "Home", 30.0));
        assert!(s.spend(id, "public:7331", "Public", 60.0));
        assert!(!s.spend(999, "x", "X", 5.0), "unknown id is ignored");
        s.get_mut(id).unwrap().pets = 3;
        let text = toml::to_string_pretty(&s).unwrap();
        assert!(text.contains("[[companions]]") && text.contains("kind = \"dog\""), "{text}");
        let back: Store = toml::from_str(&text).unwrap();
        assert_eq!(back, s);
        let dog = back.get(id).unwrap();
        assert_eq!(dog.name, "Mochi", "trimmed");
        assert_eq!(dog.coat_name(), "Black & white");
        assert_eq!(dog.collar, [1, 2, 3]);
        assert_eq!(dog.seconds_together, 180.0);
        assert_eq!(dog.visits["10.0.0.5:7331"].seconds, 120.0);
        assert!(toml::from_str::<Store>("").unwrap().companions.is_empty());
    }

    #[test]
    fn file_roundtrip_and_broken_file() {
        let path = std::env::temp_dir().join(format!("rusttalk-companions-{}.toml", std::process::id()));
        let (s, _) = store_with_dog();
        s.save_to(&path);
        assert_eq!(Store::load_from(&path), s);
        std::fs::write(&path, "not = [valid").unwrap();
        assert_eq!(Store::load_from(&path), Store::default(), "broken file -> fresh start");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn hunger_grows_with_time_together() {
        let (mut s, id) = store_with_dog();
        s.get_mut(id).unwrap().hunger = 0.0;
        s.spend(id, "a", "A", 3600.0);
        let h = s.get(id).unwrap().hunger;
        assert!((h - 0.25).abs() < 1e-3, "{h}");
        s.spend(id, "a", "A", 100.0 * 3600.0);
        assert_eq!(s.get(id).unwrap().hunger, 1.0, "capped");
    }

    #[test]
    fn slots_open_with_time_together() {
        let mut s = Store::default();
        assert!(s.can_adopt(), "the first one is free");
        let a = s.adopt(Kind::Dog, "A", 0, 0, [0, 0, 0]);
        assert!(!s.can_adopt());
        assert_eq!(s.hours_to_next_slot(), HOURS_PER_SLOT);
        s.spend(a, "x", "X", HOURS_PER_SLOT * 3600.0);
        assert!(s.can_adopt(), "a second slot after enough hours");
        let b = s.adopt(Kind::Dog, "", 0, 0, [0, 0, 0]);
        assert_ne!(a, b, "unique ids");
        assert_eq!(s.get(b).unwrap().name, NAMES[0], "empty name -> default");
    }

    #[test]
    fn genes_follow_the_identity() {
        let a = genes(&"ab".repeat(32), 0);
        assert_eq!(a, genes(&"ab".repeat(32), 0), "always the same for one user");
        assert_ne!(a, genes(&"cd".repeat(32), 0), "different users, different pets");
        assert_ne!(a, genes(&"ab".repeat(32), 1), "the second companion is another one");
    }

    #[test]
    fn seed_is_rolled_once() {
        let mut s = Store::default();
        let seed = s.seed();
        assert_eq!(seed.len(), 8);
        assert_eq!(s.seed(), seed, "stays the same");
        let back: Store = toml::from_str(&toml::to_string_pretty(&s).unwrap()).unwrap();
        assert_eq!(back.seed, s.seed);
    }

    #[test]
    fn happiness_and_bond() {
        let (mut s, id) = store_with_dog();
        let start = s.get(id).unwrap().happiness;
        assert_eq!(s.get(id).unwrap().bond_level(), (0, "Just met", 0.0));
        // Time together: the bond grows, happiness sinks without attention.
        s.get_mut(id).unwrap().hunger = 0.0;
        s.spend(id, "a", "A", 3600.0);
        let c = s.get(id).unwrap().clone();
        assert!((c.bond - (0.5 + start)).abs() < 1e-3, "{}", c.bond);
        assert!((c.happiness - (start - 0.1)).abs() < 1e-3, "{}", c.happiness);
        assert_eq!(c.bond_level().1, "Acquaintance");
        // Attention makes it happy again (never more than 1).
        let c = s.get_mut(id).unwrap();
        c.petted();
        c.fed();
        assert!((c.happiness - (start - 0.1 + 0.24)).abs() < 1e-3);
        assert_eq!((c.pets, c.snacks, c.hunger), (1, 1, 0.0));
        for _ in 0..50 {
            c.petted();
        }
        assert_eq!(c.happiness, 1.0);
        c.annoyed();
        assert!(c.happiness < 1.0);
        // Left alone for a long time: sad, hungry ones sink faster. The bond stays.
        let bond = c.bond;
        s.spend(id, "a", "A", 20.0 * 3600.0);
        let c = s.get(id).unwrap();
        assert_eq!(c.happiness, 0.0);
        assert!(c.bond > bond);
        assert_eq!(Mood::new(0.0).feel(1.0, &Companion { hunger: 0.0, ..c.clone() }), Emotion::Sad);
        assert_eq!(Mood::new(0.0).feel(1.0, c), Emotion::Hungry, "hunger shows first");
        // Old files without the new fields.
        let old: Store = toml::from_str("[[companions]]\nid = 1\nkind = \"dog\"\nname = \"A\"\ncollar = [0, 0, 0]\n").unwrap();
        assert_eq!((old.companions[0].happiness, old.companions[0].bond), (0.7, 0.0));
        let top = Companion { bond: 500.0, ..c.clone() };
        assert_eq!(top.bond_level(), (5, "Inseparable", 1.0));
    }

    #[test]
    fn names_are_cleaned() {
        assert_eq!(clean_name("  Kiko "), Some("Kiko".into()));
        assert_eq!(clean_name("   "), None);
        assert_eq!(clean_name("a\nb"), Some("ab".into()));
        assert_eq!(clean_name(&"x".repeat(50)).unwrap().len(), MAX_NAME);
        assert!(NAMES.contains(&pick(&NAMES)));
        assert!(COLLARS.contains(&pick(&COLLARS)));
    }

    #[test]
    fn mood_rules() {
        let mut m = Mood::new(0.0);
        assert_eq!(m.emotion(1.0, 0.0), Emotion::Content);
        assert_eq!(m.emotion(1.0, 0.9), Emotion::Hungry);
        assert_eq!(m.emotion(SLEEPY_AFTER + 1.0, 0.0), Emotion::Sleepy);
        assert_eq!(m.emotion(ASLEEP_AFTER + 1.0, 0.9), Emotion::Asleep, "sleep beats hunger");

        m.boop(10.0, 0.0);
        assert_eq!(m.emotion(10.5, 0.0), Emotion::Happy);
        for i in 0..5 {
            m.boop(20.0 + i as f64 * 0.3, 0.0);
        }
        assert_eq!(m.emotion(22.0, 0.0), Emotion::Angry, "too many boops");
        // Petting calms it down.
        let mut t = 22.0;
        let mut pets = 0;
        while t < 24.0 {
            t += 0.1;
            pets += m.pet(t, 0.1) as u32;
        }
        assert_eq!(pets, 1);
        assert_ne!(m.emotion(t + 0.1, 0.0), Emotion::Angry);

        // Woken by a boop: grumpy.
        let mut m = Mood::new(0.0);
        m.boop(ASLEEP_AFTER + 5.0, 0.0);
        assert_eq!(m.emotion(ASLEEP_AFTER + 6.0, 0.0), Emotion::Angry);
    }

    #[test]
    fn snacks() {
        let mut m = Mood::new(0.0);
        assert!(!m.feed(1.0, 0.05), "full");
        assert_eq!(m.emotion(1.5, 0.05), Emotion::Full);
        assert!(m.feed(10.0, 0.8));
        assert_eq!(m.emotion(10.5, 0.0), Emotion::Eating);
        assert!(m.eating_progress(11.0).is_some());
        assert_eq!(m.emotion(13.0, 0.0), Emotion::Happy, "happy after eating");
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(40.0), "40 s");
        assert_eq!(format_duration(12.0 * 60.0 + 5.0), "12 min");
        assert_eq!(format_duration(3.0 * 3600.0 + 12.0 * 60.0), "3 h 12 min");
    }
}
