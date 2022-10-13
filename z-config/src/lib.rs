#![deny(warnings)]

use serde_derive::Deserialize;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub struct MachineConfig {
    pub manufacturer: String,
    pub arch: String,
    pub user_img: Option<PathBuf>,
    pub pci_support: bool,
    pub features: Vec<String>,
}

impl MachineConfig {
    pub fn select(hardware: impl AsRef<str>) -> Option<Self> {
        type ConfigFile = HashMap<String, HashMap<String, RawHardwareConfig>>;

        #[derive(Deserialize, Debug)]
        struct RawHardwareConfig {
            arch: String,
            #[serde(rename(deserialize = "link-user-img"))]
            user_img: Option<PathBuf>,
            #[serde(rename(deserialize = "pci-support"))]
            pci_support: Option<bool>,
            features: Option<Vec<String>>,
        }

        let file = Path::new(std::env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("config")
            .join("machine-features.toml");
        let file = fs::read_to_string(file).unwrap();
        let config = toml::from_str::<ConfigFile>(&file).unwrap();
        for (manufacturer, products) in config {
            for (name, raw) in products {
                if name == hardware.as_ref() {
                    return Some(Self {
                        manufacturer,
                        arch: raw.arch,
                        user_img: raw.user_img,
                        pci_support: raw.pci_support.unwrap_or(true),
                        features: raw.features.unwrap_or_default(),
                    });
                }
            }
        }
        None
    }
}
