//! Language & Region backend owned by the settings daemon.
//!
//! Reads and writes the system locale through `localectl`: system
//! language (English/German), region (date/number/currency/measurement
//! formats) and keyboard layout (X11 layout plus optional variant, with
//! region-based auto-detect). `locale_get` and `locale_keymap_variants`
//! are public read ops; the `locale_set_*` ops are private writes
//! reserved for the Settings app (`com.tontoo.systemsettings`).

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::store::SettingsStore;

/// Store domain holding the keyboard auto-detect preference.
pub const DOMAIN: &str = "locale";
/// Region-based keyboard auto-detect key.
pub const KEY_AUTO_KEYMAP: &str = "auto_keymap";

pub const OP_LOCALE_GET: &str = "locale_get";
pub const OP_LOCALE_SET_LANGUAGE: &str = "locale_set_language";
pub const OP_LOCALE_SET_REGION: &str = "locale_set_region";
pub const OP_LOCALE_SET_KEYMAP: &str = "locale_set_keymap";
pub const OP_LOCALE_SET_AUTO_KEYMAP: &str = "locale_set_auto_keymap";
pub const OP_LOCALE_KEYMAP_VARIANTS: &str = "locale_keymap_variants";

/// System languages offered by the Settings app.
pub const LANGUAGES: &[(&str, &str)] = &[("en", "English"), ("de", "Deutsch")];
/// Locale code per language.
pub const LANGUAGE_LOCALES: &[(&str, &str)] = &[("en", "en_US.UTF-8"), ("de", "de_DE.UTF-8")];

/// Territory (ISO 3166-1 alpha-2) with English country name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionEntry {
    pub code: String,
    pub name: String,
}

/// Effective language & region state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocaleState {
    pub language: String,
    pub region: String,
    pub keymap: String,
    pub keymap_variant: Option<String>,
    pub auto_keymap: bool,
    pub languages: Vec<LanguageEntry>,
    pub regions: Vec<RegionEntry>,
    pub keymaps: Vec<String>,
}

/// Language option with display name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageEntry {
    pub code: String,
    pub name: String,
}

/// Parse `localectl status` output into `key -> value` pairs. The
/// `System Locale` line holds space-separated `KEY=value` pairs.
fn parse_status(output: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for line in output.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("System Locale:") {
            for part in rest.split_whitespace() {
                if let Some((key, value)) = part.split_once('=') {
                    map.insert(key.to_string(), value.to_string());
                }
            }
        } else if let Some((key, value)) = line.split_once(':') {
            map.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    map
}

fn live_status() -> std::collections::HashMap<String, String> {
    networkkit::util::run("localectl", &["status"])
        .map(|out| parse_status(&out))
        .unwrap_or_default()
}

/// Language code from a `LANG` value (`de_DE.UTF-8` -> `de`), mapped to
/// the offered languages with English as the default.
fn language_from_lang(value: &str) -> String {
    let code = value.split(['_', '.', '@']).next().unwrap_or("").to_lowercase();
    if LANGUAGES.iter().any(|(c, _)| *c == code) {
        code
    } else {
        "en".to_string()
    }
}

/// Territory from a locale value (`de_DE.UTF-8` -> `DE`), empty when none.
fn territory_from_locale(value: &str) -> String {
    let after_lang = value.split_once('_').map(|(_, rest)| rest).unwrap_or("");
    let territory: String = after_lang
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    if territory.len() == 2 {
        territory.to_uppercase()
    } else {
        String::new()
    }
}

fn locale_for(language: &str) -> String {
    LANGUAGE_LOCALES
        .iter()
        .find(|(c, _)| *c == language)
        .map(|(_, loc)| loc.to_string())
        .unwrap_or_else(|| "en_US.UTF-8".to_string())
}

/// X11 layouts, compact fallback when `localectl` is unavailable.
fn x11_layouts() -> Vec<String> {
    if let Ok(out) = networkkit::util::run("localectl", &["list-x11-keymap-layouts"]) {
        let layouts: Vec<String> = out
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect();
        if !layouts.is_empty() {
            return layouts;
        }
    }
    vec![
        "us", "de", "gb", "fr", "es", "it", "pt", "nl", "be", "pl", "ch",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// Variants for one X11 layout (empty when none or unavailable).
pub fn keymap_variants(layout: &str) -> Vec<String> {
    if layout.is_empty() || layout.contains(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_') {
        return Vec::new();
    }
    networkkit::util::run("localectl", &["list-x11-keymap-variants", layout])
        .map(|out| {
            out.lines()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Apply an X11 layout (plus optional variant); the console keymap is
/// best effort since console names differ from X11 names.
fn apply_keymap(layout: &str, variant: Option<&str>) -> Result<(), String> {
    match variant {
        Some(variant) => {
            networkkit::util::run("localectl", &["set-x11-keymap", layout, variant])
                .map_err(|e| format!("locale set failed: {}", e))?;
        }
        None => {
            networkkit::util::run("localectl", &["set-x11-keymap", layout])
                .map_err(|e| format!("locale set failed: {}", e))?;
        }
    }
    // Console keymap: best effort (names often coincide, variants differ).
    let _ = networkkit::util::run("localectl", &["set-keymap", layout]);
    Ok(())
}

/// Read the effective state: live `localectl` facts overlaid with the
/// stored auto-detect preference and the static option lists.
pub fn get(store: &SettingsStore) -> LocaleState {
    let status = live_status();
    let lang_value = status.get("LANG").cloned().unwrap_or_default();
    let language = language_from_lang(&lang_value);
    let mut region = status
        .get("LC_TIME")
        .map(|v| territory_from_locale(v))
        .unwrap_or_default();
    if region.is_empty() {
        region = territory_from_locale(&lang_value);
    }
    if region.is_empty() {
        region = "US".to_string();
    }
    let keymap = status.get("X11 Layout").cloned().unwrap_or_default();
    let keymap_variant = status
        .get("X11 Variant")
        .filter(|v| !v.is_empty())
        .cloned();
    LocaleState {
        language,
        region,
        keymap,
        keymap_variant,
        auto_keymap: store
            .get(DOMAIN, KEY_AUTO_KEYMAP)
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        languages: LANGUAGES
            .iter()
            .map(|(code, name)| LanguageEntry {
                code: code.to_string(),
                name: name.to_string(),
            })
            .collect(),
        regions: REGIONS
            .iter()
            .map(|(code, name)| RegionEntry {
                code: code.to_string(),
                name: name.to_string(),
            })
            .collect(),
        keymaps: x11_layouts(),
    }
}

/// Set the system language (English/German only), keeping the current
/// region formats. Returns the effective state.
pub fn set_language(
    store: &Arc<Mutex<SettingsStore>>,
    language: &str,
) -> Result<LocaleState, String> {
    if !LANGUAGES.iter().any(|(c, _)| *c == language) {
        return Err(format!("locale set failed: unsupported language {:?}", language));
    }
    let region = get_region_now();
    let base = locale_for(language);
    let lang_prefix = base.split('.').next().unwrap_or("en_US");
    let territory = if region.is_empty() {
        territory_from_locale(&base)
    } else {
        region.clone()
    };
    let regional = format!("{}_{}.UTF-8", lang_prefix.split('_').next().unwrap_or("en"), territory);
    networkkit::util::run(
        "localectl",
        &[
            "set-locale",
            &format!("LANG={}", base),
            &format!("LC_TIME={}", regional),
            &format!("LC_NUMERIC={}", regional),
            &format!("LC_MONETARY={}", regional),
            &format!("LC_MEASUREMENT={}", regional),
        ],
    )
    .map_err(|e| format!("locale set failed: {}", e))?;
    let guard = store
        .lock()
        .map_err(|_| "locale set failed: store is locked".to_string())?;
    Ok(get(&guard))
}

/// Current region territory from live `localectl` (LC_TIME, then LANG).
fn get_region_now() -> String {
    let status = live_status();
    let region = status
        .get("LC_TIME")
        .map(|v| territory_from_locale(v))
        .unwrap_or_default();
    if !region.is_empty() {
        return region;
    }
    territory_from_locale(&status.get("LANG").cloned().unwrap_or_default())
}

/// Set the region formats for the current language. With auto-detect on
/// the keyboard layout follows the region. Returns the effective state.
pub fn set_region(store: &Arc<Mutex<SettingsStore>>, region: &str) -> Result<LocaleState, String> {
    let code = region.trim().to_uppercase();
    if !REGIONS.iter().any(|(c, _)| *c == code) {
        return Err(format!("locale set failed: unknown region {:?}", region));
    }
    let status = live_status();
    let language = language_from_lang(&status.get("LANG").cloned().unwrap_or_default());
    let lang_prefix = locale_for(&language);
    let lang_prefix = lang_prefix.split('.').next().unwrap_or("en_US");
    let lang_only = lang_prefix.split('_').next().unwrap_or("en");
    let regional = format!("{}_{}.UTF-8", lang_only, code);
    networkkit::util::run(
        "localectl",
        &[
            "set-locale",
            &format!("LC_TIME={}", regional),
            &format!("LC_NUMERIC={}", regional),
            &format!("LC_MONETARY={}", regional),
            &format!("LC_MEASUREMENT={}", regional),
        ],
    )
    .map_err(|e| format!("locale set failed: {}", e))?;
    let auto = {
        let guard = store
            .lock()
            .map_err(|_| "locale set failed: store is locked".to_string())?;
        guard
            .get(DOMAIN, KEY_AUTO_KEYMAP)
            .and_then(|v| v.as_bool())
            .unwrap_or(true)
    };
    if auto {
        let layout = layout_for_region(&code);
        apply_keymap(&layout, None)?;
    }
    let guard = store
        .lock()
        .map_err(|_| "locale set failed: store is locked".to_string())?;
    Ok(get(&guard))
}

/// Set the keyboard layout (plus optional variant) and switch auto-detect
/// off. Returns the effective state.
pub fn set_keymap(
    store: &Arc<Mutex<SettingsStore>>,
    layout: &str,
    variant: Option<&str>,
) -> Result<LocaleState, String> {
    let layout = layout.trim();
    if !x11_layouts().iter().any(|l| l == layout) {
        return Err(format!("locale set failed: unknown layout {:?}", layout));
    }
    if let Some(variant) = variant {
        if !keymap_variants(layout).iter().any(|v| v == variant) {
            return Err(format!(
                "locale set failed: unknown variant {:?} for layout {:?}",
                variant, layout
            ));
        }
    }
    apply_keymap(layout, variant)?;
    let mut guard = store
        .lock()
        .map_err(|_| "locale set failed: store is locked".to_string())?;
    guard.set(DOMAIN, KEY_AUTO_KEYMAP, serde_json::json!(false));
    let effective = get(&guard);
    guard
        .save()
        .map_err(|e| format!("locale set failed: store save failed: {}", e))?;
    Ok(effective)
}

/// Switch region-based keyboard auto-detect on or off. Enabling applies
/// the mapped layout for the current region at once.
pub fn set_auto_keymap(store: &Arc<Mutex<SettingsStore>>, auto: bool) -> Result<LocaleState, String> {
    if auto {
        let region = get_region_now();
        let region = if region.is_empty() { "US".to_string() } else { region };
        apply_keymap(&layout_for_region(&region), None)?;
    }
    let mut guard = store
        .lock()
        .map_err(|_| "locale set failed: store is locked".to_string())?;
    guard.set(DOMAIN, KEY_AUTO_KEYMAP, serde_json::json!(auto));
    let effective = get(&guard);
    guard
        .save()
        .map_err(|e| format!("locale set failed: store save failed: {}", e))?;
    Ok(effective)
}

/// X11 layout for a territory (Auto Detect mapping), `us` by default.
fn layout_for_region(region: &str) -> String {
    REGION_LAYOUTS
        .iter()
        .find(|(c, _)| *c == region)
        .map(|(_, l)| l.to_string())
        .unwrap_or_else(|| "us".to_string())
}

/// Territory to X11 layout mapping for Auto Detect.
const REGION_LAYOUTS: &[(&str, &str)] = &[
    ("AT", "de"), ("AU", "us"), ("BE", "be"), ("BR", "br"), ("CA", "ca"),
    ("CH", "ch"), ("CN", "cn"), ("CZ", "cz"), ("DE", "de"), ("DK", "dk"),
    ("ES", "es"), ("FI", "fi"), ("FR", "fr"), ("GB", "gb"), ("GR", "gr"),
    ("HR", "hr"), ("HU", "hu"), ("IE", "ie"), ("IL", "il"), ("IN", "in"),
    ("IS", "is"), ("IT", "it"), ("JP", "jp"), ("KR", "kr"), ("NL", "nl"),
    ("NO", "no"), ("PL", "pl"), ("PT", "pt"), ("RO", "ro"), ("RU", "ru"),
    ("SE", "se"), ("SI", "si"), ("SK", "sk"), ("TR", "tr"), ("TW", "tw"),
    ("UA", "ua"), ("US", "us"),
    ("MX", "latam"), ("AR", "latam"), ("CL", "latam"), ("CO", "latam"),
    ("PE", "latam"), ("VE", "latam"),
    ("BG", "bg"), ("EE", "ee"), ("LV", "lv"), ("LT", "lt"), ("RS", "rs"),
    ("BA", "ba"), ("AL", "al"), ("MK", "mk"), ("BY", "by"), ("KZ", "kz"),
    ("GE", "ge"), ("AM", "am"), ("AZ", "az"), ("TH", "th"), ("VN", "vn"),
    ("ID", "id"), ("MY", "my"), ("PH", "ph"), ("SA", "sa"), ("AE", "ara"),
    ("EG", "eg"), ("ZA", "za"), ("NG", "ng"), ("KE", "ke"), ("NZ", "mao"),
];

/// Full territory list (ISO 3166-1 alpha-2) with English country names.
const REGIONS: &[(&str, &str)] = &[
    ("AF", "Afghanistan"), ("AX", "Aland Islands"), ("AL", "Albania"), ("DZ", "Algeria"),
    ("AS", "American Samoa"), ("AD", "Andorra"), ("AO", "Angola"), ("AI", "Anguilla"),
    ("AQ", "Antarctica"), ("AG", "Antigua and Barbuda"), ("AR", "Argentina"), ("AM", "Armenia"),
    ("AW", "Aruba"), ("AU", "Australia"), ("AT", "Austria"), ("AZ", "Azerbaijan"),
    ("BS", "Bahamas"), ("BH", "Bahrain"), ("BD", "Bangladesh"), ("BB", "Barbados"),
    ("BY", "Belarus"), ("BE", "Belgium"), ("BZ", "Belize"), ("BJ", "Benin"),
    ("BM", "Bermuda"), ("BT", "Bhutan"), ("BO", "Bolivia"), ("BA", "Bosnia and Herzegovina"),
    ("BW", "Botswana"), ("BV", "Bouvet Island"), ("BR", "Brazil"),
    ("IO", "British Indian Ocean Territory"), ("BN", "Brunei"), ("BG", "Bulgaria"),
    ("BF", "Burkina Faso"), ("BI", "Burundi"), ("KH", "Cambodia"), ("CM", "Cameroon"),
    ("CA", "Canada"), ("CV", "Cape Verde"), ("KY", "Cayman Islands"),
    ("CF", "Central African Republic"), ("TD", "Chad"), ("CL", "Chile"), ("CN", "China"),
    ("CX", "Christmas Island"), ("CC", "Cocos Islands"), ("CO", "Colombia"),
    ("KM", "Comoros"), ("CG", "Congo"), ("CD", "Congo (DRC)"), ("CK", "Cook Islands"),
    ("CR", "Costa Rica"), ("CI", "Cote d'Ivoire"), ("HR", "Croatia"), ("CU", "Cuba"),
    ("CW", "Curacao"), ("CY", "Cyprus"), ("CZ", "Czechia"), ("DK", "Denmark"),
    ("DJ", "Djibouti"), ("DM", "Dominica"), ("DO", "Dominican Republic"), ("EC", "Ecuador"),
    ("EG", "Egypt"), ("SV", "El Salvador"), ("GQ", "Equatorial Guinea"), ("ER", "Eritrea"),
    ("EE", "Estonia"), ("SZ", "Eswatini"), ("ET", "Ethiopia"), ("FK", "Falkland Islands"),
    ("FO", "Faroe Islands"), ("FJ", "Fiji"), ("FI", "Finland"), ("FR", "France"),
    ("GF", "French Guiana"), ("PF", "French Polynesia"), ("TF", "French Southern Territories"),
    ("GA", "Gabon"), ("GM", "Gambia"), ("GE", "Georgia"), ("DE", "Germany"),
    ("GH", "Ghana"), ("GI", "Gibraltar"), ("GR", "Greece"), ("GL", "Greenland"),
    ("GD", "Grenada"), ("GP", "Guadeloupe"), ("GU", "Guam"), ("GT", "Guatemala"),
    ("GG", "Guernsey"), ("GN", "Guinea"), ("GW", "Guinea-Bissau"), ("GY", "Guyana"),
    ("HT", "Haiti"), ("HM", "Heard Island and McDonald Islands"), ("VA", "Holy See"),
    ("HN", "Honduras"), ("HK", "Hong Kong"), ("HU", "Hungary"), ("IS", "Iceland"),
    ("IN", "India"), ("ID", "Indonesia"), ("IR", "Iran"), ("IQ", "Iraq"),
    ("IE", "Ireland"), ("IM", "Isle of Man"), ("IL", "Israel"), ("IT", "Italy"),
    ("JM", "Jamaica"), ("JP", "Japan"), ("JE", "Jersey"), ("JO", "Jordan"),
    ("KZ", "Kazakhstan"), ("KE", "Kenya"), ("KI", "Kiribati"), ("KP", "North Korea"),
    ("KR", "South Korea"), ("KW", "Kuwait"), ("KG", "Kyrgyzstan"), ("LA", "Laos"),
    ("LV", "Latvia"), ("LB", "Lebanon"), ("LS", "Lesotho"), ("LR", "Liberia"),
    ("LY", "Libya"), ("LI", "Liechtenstein"), ("LT", "Lithuania"), ("LU", "Luxembourg"),
    ("MO", "Macao"), ("MG", "Madagascar"), ("MW", "Malawi"), ("MY", "Malaysia"),
    ("MV", "Maldives"), ("ML", "Mali"), ("MT", "Malta"), ("MH", "Marshall Islands"),
    ("MQ", "Martinique"), ("MR", "Mauritania"), ("MU", "Mauritius"), ("YT", "Mayotte"),
    ("MX", "Mexico"), ("FM", "Micronesia"), ("MD", "Moldova"), ("MC", "Monaco"),
    ("MN", "Mongolia"), ("ME", "Montenegro"), ("MS", "Montserrat"), ("MA", "Morocco"),
    ("MZ", "Mozambique"), ("MM", "Myanmar"), ("NA", "Namibia"), ("NR", "Nauru"),
    ("NP", "Nepal"), ("NL", "Netherlands"), ("NC", "New Caledonia"), ("NZ", "New Zealand"),
    ("NI", "Nicaragua"), ("NE", "Niger"), ("NG", "Nigeria"), ("NU", "Niue"),
    ("NF", "Norfolk Island"), ("MK", "North Macedonia"), ("MP", "Northern Mariana Islands"),
    ("NO", "Norway"), ("OM", "Oman"), ("PK", "Pakistan"), ("PW", "Palau"),
    ("PS", "Palestine"), ("PA", "Panama"), ("PG", "Papua New Guinea"), ("PY", "Paraguay"),
    ("PE", "Peru"), ("PH", "Philippines"), ("PN", "Pitcairn"), ("PL", "Poland"),
    ("PT", "Portugal"), ("PR", "Puerto Rico"), ("QA", "Qatar"), ("RE", "Reunion"),
    ("RO", "Romania"), ("RU", "Russia"), ("RW", "Rwanda"), ("BL", "Saint Barthelemy"),
    ("SH", "Saint Helena"), ("KN", "Saint Kitts and Nevis"), ("LC", "Saint Lucia"),
    ("MF", "Saint Martin"), ("PM", "Saint Pierre and Miquelon"),
    ("VC", "Saint Vincent and the Grenadines"), ("WS", "Samoa"), ("SM", "San Marino"),
    ("ST", "Sao Tome and Principe"), ("SA", "Saudi Arabia"), ("SN", "Senegal"),
    ("RS", "Serbia"), ("SC", "Seychelles"), ("SL", "Sierra Leone"), ("SG", "Singapore"),
    ("SX", "Sint Maarten"), ("SK", "Slovakia"), ("SI", "Slovenia"), ("SB", "Solomon Islands"),
    ("SO", "Somalia"), ("ZA", "South Africa"),
    ("GS", "South Georgia and the South Sandwich Islands"), ("SS", "South Sudan"),
    ("ES", "Spain"), ("LK", "Sri Lanka"), ("SD", "Sudan"), ("SR", "Suriname"),
    ("SJ", "Svalbard and Jan Mayen"), ("SE", "Sweden"), ("CH", "Switzerland"),
    ("SY", "Syria"), ("TW", "Taiwan"), ("TJ", "Tajikistan"), ("TZ", "Tanzania"),
    ("TH", "Thailand"), ("TL", "Timor-Leste"), ("TG", "Togo"), ("TK", "Tokelau"),
    ("TO", "Tonga"), ("TT", "Trinidad and Tobago"), ("TN", "Tunisia"), ("TR", "Turkey"),
    ("TM", "Turkmenistan"), ("TC", "Turks and Caicos Islands"), ("TV", "Tuvalu"),
    ("UG", "Uganda"), ("UA", "Ukraine"), ("AE", "United Arab Emirates"),
    ("GB", "United Kingdom"), ("US", "United States"),
    ("UM", "United States Minor Outlying Islands"), ("UY", "Uruguay"), ("UZ", "Uzbekistan"),
    ("VU", "Vanuatu"), ("VE", "Venezuela"), ("VN", "Vietnam"),
    ("VG", "Virgin Islands (British)"), ("VI", "Virgin Islands (U.S.)"),
    ("WF", "Wallis and Futuna"), ("EH", "Western Sahara"), ("YE", "Yemen"),
    ("ZM", "Zambia"), ("ZW", "Zimbabwe"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn memory_store() -> Arc<Mutex<SettingsStore>> {
        Arc::new(Mutex::new(SettingsStore::new(PathBuf::from(
            "/tmp/tontoo-settings-locale-test.json",
        ))))
    }

    #[test]
    fn regions_are_unique_and_sorted() {
        assert!(REGIONS.len() > 200);
        let mut codes: Vec<&str> = REGIONS.iter().map(|(c, _)| *c).collect();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), REGIONS.len());
        assert!(REGIONS.iter().all(|(c, n)| c.len() == 2 && !n.is_empty()));
    }

    #[test]
    fn status_parses_system_locale_pairs() {
        let out = "System Locale: LANG=de_DE.UTF-8 LC_TIME=de_DE.UTF-8\n   VC Keymap: de\n  X11 Layout: de\n X11 Variant: nodeadkeys\n";
        let map = parse_status(out);
        assert_eq!(map.get("LANG").map(String::as_str), Some("de_DE.UTF-8"));
        assert_eq!(map.get("LC_TIME").map(String::as_str), Some("de_DE.UTF-8"));
        assert_eq!(map.get("X11 Layout").map(String::as_str), Some("de"));
        assert_eq!(map.get("X11 Variant").map(String::as_str), Some("nodeadkeys"));
    }

    #[test]
    fn language_and_territory_parsing() {
        assert_eq!(language_from_lang("de_DE.UTF-8"), "de");
        assert_eq!(language_from_lang("en_US.UTF-8"), "en");
        assert_eq!(language_from_lang("C.UTF-8"), "en");
        assert_eq!(territory_from_locale("de_DE.UTF-8"), "DE");
        assert_eq!(territory_from_locale("en_US.UTF-8"), "US");
        assert_eq!(territory_from_locale("C.UTF-8"), "");
    }

    #[test]
    fn layout_mapping_defaults_to_us() {
        assert_eq!(layout_for_region("DE"), "de");
        assert_eq!(layout_for_region("CH"), "ch");
        assert_eq!(layout_for_region("MX"), "latam");
        assert_eq!(layout_for_region("XX"), "us");
    }

    #[test]
    fn rejects_bad_language_region_layout() {
        let store = memory_store();
        assert!(set_language(&store, "fr").is_err());
        assert!(set_language(&store, "").is_err());
        assert!(set_region(&store, "XX").is_err());
        assert!(set_region(&store, "").is_err());
        assert!(set_keymap(&store, "not-a-layout", None).is_err());
        assert!(set_keymap(&store, "", None).is_err());
    }

    #[test]
    fn get_returns_full_option_lists() {
        let store = memory_store();
        let guard = store.lock().unwrap();
        let state = get(&guard);
        assert_eq!(state.languages.len(), 2);
        assert!(state.regions.len() > 200);
        assert!(!state.keymaps.is_empty());
        assert!(state.auto_keymap);
    }
}
