//! GUI-local translation catalogs.
//!
//! Keeping the catalog at the UI boundary means a new language only needs a
//! new `UiText` value here. Slint components consume the typed catalog through
//! the `I18n` global and do not contain locale conditionals.

use std::cell::RefCell;

use obs_rs_ui::UiLocale;
use slint::{ComponentHandle, SharedString};

use crate::{I18n, MainWindow, UiText};

mod english;
mod spanish;

thread_local! {
    /// Catalogs are built once per thread and then reused.
    ///
    /// Constructing a `UiText` allocates roughly 150 strings. The refresh tick
    /// asked for a catalog several times per frame, so at 30 fps that was
    /// thousands of string allocations per second for data that only changes
    /// when the user switches language.
static ENGLISH_CATALOG: UiText = english::catalog();
    static SPANISH_CATALOG: UiText = spanish::catalog();
    static APPLIED_LOCALE: RefCell<Option<(usize, UiLocale)>> = const { RefCell::new(None) };
}

/// Applies the complete catalog for `locale` to the live Slint tree.
pub(crate) fn apply(ui: &MainWindow, locale: UiLocale) {
    ui.global::<I18n>().set_text(catalog(locale));
}

/// Applies a catalog only when this component tree changes locale.
pub(crate) fn apply_if_changed(ui: &MainWindow, locale: UiLocale) {
    let key = std::ptr::from_ref(ui) as usize;
    let changed = APPLIED_LOCALE.with(|applied| {
        let mut applied = applied.borrow_mut();
        if *applied == Some((key, locale)) {
            false
        } else {
            *applied = Some((key, locale));
            true
        }
    });
    if changed {
        apply(ui, locale);
    }
}

/// Returns the catalog for a supported locale.
///
/// This clones the cached catalog, which copies `SharedString` handles rather
/// than string data. Call sites that only read a field or two should prefer
/// [`with_catalog`], which clones nothing.
#[must_use]
pub(crate) fn catalog(locale: UiLocale) -> UiText {
    with_catalog(locale, Clone::clone)
}

/// Runs `read` against the cached catalog for `locale` without cloning it.
pub(crate) fn with_catalog<R>(locale: UiLocale, read: impl FnOnce(&UiText) -> R) -> R {
    match locale {
        UiLocale::English => ENGLISH_CATALOG.with(read),
        UiLocale::Spanish => SPANISH_CATALOG.with(read),
    }
}

fn s(value: &str) -> SharedString {
    value.into()
}
