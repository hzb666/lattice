// Discover combines alphaXiv full text, PubMed biomedical records, and
// OpenAlex's broader scholarly catalogue.

use crate::alphaxiv;
use crate::models::{LiteratureHit, OpenAlexWork};
use crate::openalex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// One page of Discover results. `page` is zero-indexed; alphaXiv is available
/// on page zero only. The client deduplicates results across pages.
pub fn search(query: &str, precise: bool, page: u32) -> Result<LiteraturePage, String> {
    let (pubmed, open, alpha) = std::thread::scope(|scope| {
        let pubmed = scope.spawn(|| crate::pubmed::search_works(query, precise, page));
        let open = scope.spawn(|| openalex::search_works(query, precise, page.saturating_add(1)));
        let alpha = (page == 0).then(|| scope.spawn(|| alphaxiv::search_works(query)));
        (pubmed.join(), open.join(), alpha.map(|task| task.join()))
    });
    let pubmed = pubmed.unwrap_or_else(|_| Err("PubMed search stopped unexpectedly.".into()));
    let open = open.unwrap_or_else(|_| Err("OpenAlex search stopped unexpectedly.".into()));
    let alpha = alpha
        .map(|result| {
            result.unwrap_or_else(|_| Err("alphaXiv search stopped unexpectedly.".into()))
        })
        .unwrap_or_else(|| Ok(Vec::new()));
    let (open, open_has_more) = match open {
        Ok(open) => {
            let has_more = open.len() as u32 >= openalex::PER_PAGE;
            (open, has_more)
        }
        Err(error) => {
            log::warn!(target: "lattice::literature", "OpenAlex search unavailable: {error}");
            (Vec::new(), false)
        }
    };
    let (pubmed, pubmed_has_more, pubmed_unavailable) = match pubmed {
        Ok(page) => (page.hits, page.has_more, false),
        Err(error) => {
            log::warn!(target: "lattice::literature", "PubMed search unavailable: {error}");
            (Vec::new(), false, true)
        }
    };
    let hits = merge_available(pubmed, alpha, open);
    let has_more = pubmed_has_more || open_has_more;
    Ok(LiteraturePage { hits, has_more, pubmed_unavailable })
}

fn merge_available(
    pubmed: Vec<LiteratureHit>, alpha: Result<Vec<LiteratureHit>, String>, open: Vec<OpenAlexWork>,
) -> Vec<LiteratureHit> {
    let alpha = alpha.unwrap_or_else(|error| {
        log::warn!(target: "lattice::literature", "alphaXiv search unavailable: {error}");
        Vec::new()
    });
    merge(pubmed, alpha, open)
}

fn merge(
    pubmed: Vec<LiteratureHit>, alpha: Vec<LiteratureHit>, open: Vec<OpenAlexWork>,
) -> Vec<LiteratureHit> {
    let mut hits: Vec<LiteratureHit> = Vec::with_capacity(alpha.len() + pubmed.len() + open.len());
    let mut seen = HashSet::new();

    for hit in pubmed.into_iter().chain(alpha).chain(open.into_iter().map(from_openalex)) {
        let identities = hit.identity_keys();
        if identities.iter().any(|identity| seen.contains(identity)) {
            if let Some(existing) = hits.iter_mut().find(|existing| {
                existing.identity_keys().iter().any(|identity| identities.contains(identity))
            }) {
                if existing.arxiv_id.is_none() {
                    existing.arxiv_id = hit.arxiv_id;
                }
                if existing.doi.is_none() {
                    existing.doi = hit.doi;
                }
                if existing.external_id.is_none() {
                    existing.external_id = hit.external_id;
                }
            }
            seen.extend(identities);
            continue;
        }
        seen.extend(identities);
        hits.push(hit);
    }
    hits
}

fn from_openalex(work: OpenAlexWork) -> LiteratureHit {
    LiteratureHit {
        source: "openalex".to_string(),
        arxiv_id: work.arxiv_id,
        title: work.title,
        year: work.year,
        authors: work.authors,
        cited_by_count: Some(work.cited_by_count),
        votes: None,
        snippet: None,
        r#abstract: work.r#abstract,
        doi: work.doi,
        landing_url: work.landing_url,
        venue: work.venue,
        external_id: None,
    }
}

/// One page of Discover results. `has_more` means another OpenAlex or PubMed
/// page can be fetched; alphaXiv is exhausted after page 0.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiteraturePage {
    pub hits: Vec<LiteratureHit>,
    pub has_more: bool,
    #[serde(default)]
    pub pubmed_unavailable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// An alphaXiv row as `alphaxiv::search_works` maps it.
    fn alpha(arxiv_id: &str, title: &str) -> LiteratureHit {
        let hit = json!({"source": "alphaxiv", "arxivId": arxiv_id, "title": title, "authors": []});
        serde_json::from_value(hit).unwrap()
    }

    fn open(arxiv: Option<&str>, title: &str) -> OpenAlexWork {
        let work = json!({"id": title, "title": title, "citedByCount": 3, "arxivId": arxiv, "authors": []});
        serde_json::from_value(work).unwrap()
    }

    fn open_doi(doi: &str, title: &str) -> OpenAlexWork {
        let work = json!({"id": title, "title": title, "citedByCount": 3, "doi": doi, "authors": []});
        serde_json::from_value(work).unwrap()
    }

    fn open_with_ids(arxiv: &str, doi: &str, title: &str) -> OpenAlexWork {
        let work = json!({"id": title, "title": title, "citedByCount": 3, "arxivId": arxiv, "doi": doi, "authors": []});
        serde_json::from_value(work).unwrap()
    }

    fn hit(source: &str, doi: Option<&str>, title: &str) -> LiteratureHit {
        serde_json::from_value(
            json!({ "source": source, "doi": doi, "title": title, "authors": [] }),
        )
        .unwrap()
    }

    /// PubMed ranks first; stable identifiers deduplicate equivalent records
    /// without collapsing separate works that share a title.
    #[test]
    fn merges_sources_by_title_and_identifier_and_survives_optional_source_failure() {
        for (alpha_hits, pubmed_hits, open_hits, expected) in [
            (
                Ok(vec![alpha("2401.00001", "Alpha One")]),
                vec![],
                vec![open(None, "No arXiv here"), open(Some("2402.00002"), "Open Two")],
                vec![
                    ("alphaxiv", "Alpha One"),
                    ("openalex", "No arXiv here"),
                    ("openalex", "Open Two"),
                ],
            ),
            (
                Ok(vec![alpha("2401.00001v2", "Alpha One")]),
                vec![],
                vec![open(Some("2401.00001"), "Same Paper From OpenAlex")],
                vec![("alphaxiv", "Alpha One")],
            ),
            (
                Ok(vec![]),
                vec![hit("pubmed", Some("10.1234/example"), "Same Paper")],
                vec![open_doi("https://doi.org/10.1234/EXAMPLE", "Different spelling")],
                vec![("pubmed", "Same Paper")],
            ),
            (
                Ok(vec![]),
                vec![
                    hit("pubmed", Some("10.1234/first"), "Shared title"),
                    hit("pubmed", Some("10.1234/second"), "Shared title"),
                ],
                vec![],
                vec![("pubmed", "Shared title"), ("pubmed", "Shared title")],
            ),
            (
                Ok(vec![]),
                vec![hit("pubmed", Some("10.1234/bridge"), "Biomedical title")],
                vec![open_doi("10.1234/bridge", "Bridge record")],
                vec![("pubmed", "Biomedical title")],
            ),
            (
                Err("Could not parse alphaXiv response".to_string()),
                vec![hit("pubmed", None, "PubMed result")],
                vec![open(Some("2402.00002"), "Open Two")],
                vec![("pubmed", "PubMed result"), ("openalex", "Open Two")],
            ),
        ] {
            let hits = merge_available(pubmed_hits, alpha_hits, open_hits);
            let got: Vec<_> = hits.iter().map(|h| (h.source.as_str(), h.title.as_str())).collect();
            assert_eq!(got, expected);
        }

        let hits = merge(
            vec![hit("pubmed", Some("10.1234/bridge"), "Biomedical title")],
            Vec::new(),
            vec![open_with_ids("2401.00001", "10.1234/bridge", "Bridge record")],
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].arxiv_id.as_deref(), Some("2401.00001"));
    }
}
