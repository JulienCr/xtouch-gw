//! Page navigation and management

use crate::config::PageConfig;
use anyhow::{anyhow, Result};
use std::collections::HashSet;
use tracing::info;

impl super::Router {
    /// Get the active page configuration
    pub async fn get_active_page(&self) -> Option<PageConfig> {
        let config = self.config.read().await;
        let index = *self.active_page_index.read().await;
        config.pages.get(index).cloned()
    }

    /// Get the active page name
    pub async fn get_active_page_name(&self) -> String {
        self.get_active_page()
            .await
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "(none)".to_string())
    }

    /// Get all apps that are active on the current page
    ///
    /// This is a convenience wrapper around `get_apps_for_page` that uses
    /// the currently active page. Returns an empty set if no page is active.
    ///
    /// Used by main.rs to filter state updates to only apps on the active page.
    pub async fn get_apps_for_active_page(&self) -> HashSet<String> {
        let config = self.config.read().await;
        let index = *self.active_page_index.read().await;

        match config.pages.get(index) {
            Some(page) => self.get_apps_for_page(page, &config),
            None => HashSet::new(),
        }
    }

    /// List all page names
    pub async fn list_pages(&self) -> Vec<String> {
        let config = self.config.read().await;
        config.pages.iter().map(|p| p.name.clone()).collect()
    }

    /// Set active page by index or name
    pub async fn set_active_page(&self, name_or_index: &str) -> Result<()> {
        let config = self.config.read().await;

        // Try parsing as index first
        if let Ok(index) = name_or_index.parse::<usize>() {
            if index < config.pages.len() {
                *self.active_page_index.write().await = index;
                let page_name = self.get_active_page_name().await;
                info!("Active page: {}", page_name);
                drop(config); // Release lock before refresh
                self.refresh_page().await;
                return Ok(());
            }
            return Err(anyhow!("Page index {} out of range", index));
        }

        // Try finding by name
        if let Some(index) = config
            .pages
            .iter()
            .position(|p| p.name.eq_ignore_ascii_case(name_or_index))
        {
            *self.active_page_index.write().await = index;
            let page_name = self.get_active_page_name().await;
            info!("Active page: {}", page_name);
            drop(config); // Release lock before refresh
            self.refresh_page().await;
            return Ok(());
        }

        Err(anyhow!("Page '{}' not found", name_or_index))
    }

    /// Navigate to the next page (circular)
    pub async fn next_page(&self) {
        let config = self.config.read().await;
        if config.pages.is_empty() {
            return;
        }

        let mut index = self.active_page_index.write().await;
        *index = (*index + 1) % config.pages.len();
        let page_name = config.pages[*index].name.clone();
        info!("Next page → {}", page_name);
        drop(index);
        drop(config);

        self.refresh_page().await;
    }

    /// Navigate to the previous page (circular)
    pub async fn prev_page(&self) {
        let config = self.config.read().await;
        if config.pages.is_empty() {
            return;
        }

        let mut index = self.active_page_index.write().await;
        *index = if *index == 0 {
            config.pages.len() - 1
        } else {
            *index - 1
        };
        let page_name = config.pages[*index].name.clone();
        info!("Previous page → {}", page_name);
        drop(index);
        drop(config);

        self.refresh_page().await;
    }

    /// Get all apps that are active on a given page
    ///
    /// This includes apps referenced in:
    /// 1. Page-specific controls (`page.controls.*.app`)
    /// 2. Global controls (`pages_global.controls.*.app`)
    /// 3. Passthrough configurations (TODO)
    ///
    /// Used for page-aware feedback filtering (matches TypeScript getAppsForPage)
    pub(crate) fn get_apps_for_page(&self, page: &crate::config::PageConfig, config: &crate::config::AppConfig) -> HashSet<String> {
        let mut apps = HashSet::new();

        // 1. Extract apps from page-specific controls
        if let Some(controls) = &page.controls {
            for (_, mapping) in controls {
                apps.insert(mapping.app.clone());
            }
        }

        // 2. Extract apps from global controls (always available on all pages)
        if let Some(global) = &config.pages_global {
            if let Some(controls) = &global.controls {
                for (_, mapping) in controls {
                    apps.insert(mapping.app.clone());
                }
            }
        }

        // 3. TODO: Extract apps from passthrough configurations
        // if let Some(passthroughs) = &page.passthroughs {
        //     for pt in passthroughs {
        //         apps.insert(pt.app.clone());
        //     }
        // }

        apps
    }
}

