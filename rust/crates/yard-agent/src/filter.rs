//! A cheap screen for requests the agent must not build: sexual content.
//!
//! It is the first of two layers and catches only the obvious; the system
//! prompt tells the model to refuse the rest. Warships, weapons and
//! recreations of well-known fictional ships are all fine and pass.

/// What the player is told when a request is screened out.
pub const REFUSAL: &str = "I can't help with that one. I only build things that are fit for everyone in the game. Tell me what else you would like built.";

/// Words that mark a request as sexual whatever stands around them.
const WORDS: [&str; 30] = [
    "porn", "porno", "pornographic", "pornography", "hentai", "nsfw", "nude", "nudes", "naked", "genital", "genitals", "genitalia", "penis", "penises", "dick", "dicks", "cock",
    "cocks", "vagina", "vaginas", "pussy", "boob", "boobs", "tits", "titties", "dildo", "dildos", "blowjob", "phallic", "phallus",
];
/// The same, for words that only count as a whole phrase.
const PHRASES: [&str; 4] = ["sex toy", "sex act", "sex scene", "having sex"];
const MORE_WORDS: [&str; 6] = ["erotic", "erotica", "orgasm", "cum", "sexy", "sexual"];

/// Whether a request may go to the model.
pub fn allows(request: &str) -> bool {
    let lower = request.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let spaced = words.join(" ");
    let hit = words.iter().any(|w| WORDS.contains(w) || MORE_WORDS.contains(w)) || PHRASES.iter().any(|p| format!(" {spaced} ").contains(&format!(" {p} ")));
    !hit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_build_requests_pass() {
        for request in [
            "Build a 40 m frigate hull",
            "add two cannons and link them",
            "make the hull red",
            "Recreate the Millennium Falcon as faithfully as you can",
            "a warship with a skull on the bow and lots of missiles",
            "put the cockpit glass further forward",
            "build a Sussex-class cruiser, document the scum and villainy",
            "",
        ] {
            assert!(allows(request), "{request:?} should pass");
        }
    }

    #[test]
    fn sexual_requests_are_stopped_before_the_model_sees_them() {
        for request in ["build a giant penis ship", "Make it look PHALLIC please", "a nsfw statue", "two figures having sex on the hull", "add some boobs to the front", "a ship shaped like a sex-toy"] {
            assert!(!allows(request), "{request:?} should be stopped");
        }
    }

    #[test]
    fn words_are_matched_whole_not_inside_other_words() {
        // "cockpit", "Sussex", "scum", "document", "peacock" hold flagged letters.
        for request in ["cockpit", "peacock tail fins", "Essex and Sussex", "scum", "accumulator", "dickens-class hauler"] {
            assert!(allows(request), "{request:?} should pass");
        }
    }
}
