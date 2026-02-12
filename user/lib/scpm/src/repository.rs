//! Package repository support

use alloc::{string::String, vec::Vec};

/// Repository index entry
#[derive(Debug, Clone)]
pub struct RepoEntry {
    pub name: String,
    pub version: String,
    pub description: String,
}

/// Package repository
#[derive(Debug)]
pub struct Repository {
    pub url: String,
    pub packages: Vec<RepoEntry>,
}

impl Repository {
    pub fn new(url: String) -> Self {
        Self {
            url,
            packages: Vec::new(),
        }
    }

    pub fn search(&self, query: &str) -> Vec<&RepoEntry> {
        self.packages
            .iter()
            .filter(|pkg| {
                let name_lower = pkg.name.to_lowercase();
                let desc_lower = pkg.description.to_lowercase();
                let query_lower = query.to_lowercase();
                name_lower.contains(&query_lower) || desc_lower.contains(&query_lower)
            })
            .collect()
    }

    pub fn get_package(&self, name: &str) -> Option<&RepoEntry> {
        self.packages.iter().find(|pkg| pkg.name == name)
    }
}

#[derive(Debug)]
pub struct RepositoryIndex {
    pub repositories: Vec<Repository>,
}

impl Default for RepositoryIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl RepositoryIndex {
    pub fn new() -> Self {
        Self {
            repositories: Vec::new(),
        }
    }

    pub fn add_repository(&mut self, repo: Repository) {
        self.repositories.push(repo);
    }

    pub fn search_all(&self, query: &str) -> Vec<&RepoEntry> {
        let mut results = Vec::new();
        for repo in &self.repositories {
            results.extend(repo.search(query));
        }
        results
    }

    pub fn get_package(&self, name: &str) -> Option<&RepoEntry> {
        for repo in &self.repositories {
            if let Some(pkg) = repo.get_package(name) {
                return Some(pkg);
            }
        }
        None
    }
}
