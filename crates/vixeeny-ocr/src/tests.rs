use std::cell::RefCell;
use std::collections::BTreeMap;

use super::*;

/// Returns canned lines per language and records the calls.
struct Fake {
    installed: Vec<&'static str>,
    output: BTreeMap<&'static str, Result<Vec<&'static str>, &'static str>>,
    calls: RefCell<Vec<(String, u32)>>,
    max: u32,
}

impl Fake {
    fn new(installed: &[&'static str]) -> Self {
        Self {
            installed: installed.to_vec(),
            output: BTreeMap::new(),
            calls: RefCell::new(vec![]),
            max: 10_000,
        }
    }

    fn says(mut self, tag: &'static str, lines: &[&'static str]) -> Self {
        self.output.insert(tag, Ok(lines.to_vec()));
        self
    }
}

impl Engine for Fake {
    fn installed_languages(&self) -> Result<Vec<Language>, OcrError> {
        Ok(self
            .installed
            .iter()
            .map(|t| Language {
                tag: (*t).to_owned(),
                display_name: (*t).to_owned(),
            })
            .collect())
    }

    fn max_dimension(&self) -> u32 {
        self.max
    }

    fn recognize(&self, image: &OcrImage, language: &str) -> Result<Vec<String>, OcrError> {
        self.calls
            .borrow_mut()
            .push((language.to_owned(), image.width));
        match self.output.get(language) {
            Some(Ok(lines)) => Ok(lines.iter().map(|l| (*l).to_owned()).collect()),
            Some(Err(e)) => Err(OcrError::Engine((*e).to_owned())),
            None => Ok(vec![]),
        }
    }
}

fn img(w: u32) -> OcrImage {
    OcrImage::new(w, 2, vec![0; w as usize * 8]).unwrap_or_else(|| unreachable!())
}

fn v(s: &[&str]) -> Vec<String> {
    s.iter().map(|x| (*x).to_owned()).collect()
}

#[test]
fn the_best_fitting_language_wins() {
    // an image of Japanese text read by both engines: the Latin engine hallucinates a little
    let engine = Fake::new(&["en-US", "ja"])
        .says("en-US", &["l1 ll"])
        .says("ja", &["日本語の", "テキスト"]);
    let r = recognize(&engine, img(4), &v(&["en", "ja"]), "en").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        (r.language.as_str(), r.text.as_str()),
        ("ja", "日本語の\nテキスト")
    );
    assert!(r.missing.is_empty());
}

#[test]
fn ties_go_to_the_first_requested_language() {
    let engine = Fake::new(&["en-US", "ja"])
        .says("en-US", &["Hello world"])
        .says("ja", &["Hello world"]);
    let r = recognize(&engine, img(4), &v(&["en", "ja"]), "en").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(r.language, "en-US");
    let r = recognize(&engine, img(4), &v(&["ja", "en"]), "en").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(r.language, "ja");
}

#[test]
fn missing_languages_are_reported_next_to_a_result() {
    let engine = Fake::new(&["en-US"]).says("en-US", &["Hello"]);
    let r = recognize(&engine, img(4), &v(&["en", "ko"]), "en").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(r.missing, ["ko"]);
}

#[test]
fn no_installed_language_is_a_clear_error_not_a_crash() {
    let engine = Fake::new(&["en-US"]);
    let err = recognize(&engine, img(4), &v(&["ja", "ko"]), "fr").unwrap_err();
    assert_eq!(
        err,
        OcrError::NoLanguage {
            missing: v(&["ja", "ko"]),
            installed: v(&["en-US"])
        }
    );
    assert!(err.to_string().contains("ja, ko"));
    assert!(
        engine.calls.borrow().is_empty(),
        "no engine call without a usable language"
    );
}

#[test]
fn oversized_images_are_scaled_for_the_engine() {
    let mut engine = Fake::new(&["en-US"]).says("en-US", &["x"]);
    engine.max = 4;
    recognize(&engine, img(10), &v(&["en"]), "en").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(engine.calls.borrow()[0].1, 3); // 10 / ceil(10/4)=3 → 3
}

#[test]
fn an_engine_failure_in_one_language_does_not_hide_the_others() {
    let mut engine = Fake::new(&["en-US", "de-DE"]).says("de-DE", &["Größe"]);
    engine.output.insert("en-US", Err("boom"));
    let r = recognize(&engine, img(4), &v(&["en", "de"]), "en").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(r.language, "de-DE");
    // all failing → the last error
    let mut all_bad = Fake::new(&["en-US"]);
    all_bad.output.insert("en-US", Err("boom"));
    assert_eq!(
        recognize(&all_bad, img(4), &v(&["en"]), "en").unwrap_err(),
        OcrError::Engine("boom".into())
    );
}

#[test]
fn empty_text_is_a_valid_result() {
    let engine = Fake::new(&["en-US"]);
    let r = recognize(&engine, img(4), &v(&["auto"]), "en").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(r.text, "");
}
