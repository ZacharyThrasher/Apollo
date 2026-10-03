//! YouTube client identity (impersonation) for the metadata endpoints. Constants live in the
//! bundled `clients.json` so a version rotation is a data change.

use std::collections::HashMap;

use serde::Deserialize;

/// A bag of identity strings + feature flags. context/02.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct YouTubeClient {
    /// Goes in `context.client.clientName` and is the string name.
    pub client_name: String,
    pub client_version: String,
    /// The NUMERIC id → `X-YouTube-Client-Name` header (as a string).
    pub client_id: String,
    pub user_agent: String,

    #[serde(default)]
    pub os_name: Option<String>,
    #[serde(default)]
    pub os_version: Option<String>,
    #[serde(default)]
    pub device_make: Option<String>,
    #[serde(default)]
    pub device_model: Option<String>,
    #[serde(default)]
    pub android_sdk_version: Option<String>,
    #[serde(default)]
    pub build_id: Option<String>,
    #[serde(default)]
    pub cronet_version: Option<String>,
    #[serde(default)]
    pub package_name: Option<String>,
    #[serde(default)]
    pub friendly_name: Option<String>,

    #[serde(default)]
    pub login_supported: bool,
    #[serde(default)]
    pub login_required: bool,
    #[serde(default)]
    pub use_signature_timestamp: bool,
    #[serde(default)]
    pub is_embedded: bool,
    /// Web client: needs PoToken + n-transform (deferred to Phase 2).
    #[serde(default)]
    pub use_web_po_tokens: bool,
}

const BUNDLED: &str = include_str!("../clients.json");

/// The client registry, loaded once at startup. Only `WEB_REMIX` is bundled.
#[derive(Debug, Clone)]
pub struct Clients(HashMap<String, YouTubeClient>);

impl Clients {
    /// Parse the bundled `clients.json`. Panics only on a corrupt bundled asset (a build bug).
    pub fn bundled() -> Self {
        Clients(serde_json::from_str(BUNDLED).expect("bundled clients.json is valid"))
    }

    /// Parse a caller-supplied override (app data dir). Falls back to bundled on error.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        Ok(Clients(serde_json::from_str(json)?))
    }

    /// Look up a client by its registry key (e.g. `"VISIONOS"`, `"ANDROID_VR_1_43_32"`).
    pub fn get(&self, key: &str) -> Option<&YouTubeClient> {
        self.0.get(key)
    }
}

/// The metadata client for search/browse/next (renderer shape only comes back as WEB_REMIX).
/// Apollo resolves audio with yt-dlp, so none of Limusic's stream or lyrics clients are bundled.
pub const METADATA_CLIENT: &str = "WEB_REMIX";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_clients_parse() {
        let clients = Clients::bundled();
        let web = clients.get(METADATA_CLIENT).expect("metadata client");
        assert!(web.login_supported);
    }

    #[test]
    fn client_numeric_ids_are_strings() {
        assert_eq!(Clients::bundled().get("WEB_REMIX").unwrap().client_id, "67");
    }
}
