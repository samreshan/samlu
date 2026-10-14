//! Whichever model is supplying meaning, behind one interface: the indexer
//! embeds passages and searches embed queries without knowing which.

use super::embed::{self, Choice};
use super::gemma;
use model2vec_rs::model::StaticModel;
use std::path::Path;
use std::sync::Arc;

/// How many passages go to EmbeddingGemma in one request.
const GEMMA_BATCH: usize = 8;

#[derive(Clone)]
pub enum Engine {
    Small(Arc<StaticModel>),
    Large(Arc<gemma::Server>),
}

impl Engine {
    /// Loads the chosen model: the small one from disk, the large one by
    /// starting its llama.cpp helper.
    pub fn load(choice: Choice, root: &Path) -> Result<Option<Self>, String> {
        match choice {
            Choice::None => Ok(None),
            Choice::Small if embed::is_downloaded(root) => {
                Ok(Some(Engine::Small(Arc::new(embed::load(root)?))))
            }
            Choice::Large if gemma::is_downloaded(root) => {
                Ok(Some(Engine::Large(Arc::new(gemma::Server::start(root)?))))
            }
            _ => Ok(None),
        }
    }

    pub fn choice(&self) -> Choice {
        match self {
            Engine::Small(_) => Choice::Small,
            Engine::Large(_) => Choice::Large,
        }
    }

    /// Stored with the index: vectors made by another model are made again.
    pub fn source(&self) -> &'static str {
        match self {
            Engine::Small(_) => embed::VECTOR_SOURCE,
            Engine::Large(_) => gemma::VECTOR_SOURCE,
        }
    }

    /// False once a helper process has gone away; it is started again.
    pub fn is_alive(&self) -> bool {
        match self {
            Engine::Small(_) => true,
            Engine::Large(server) => server.is_alive(),
        }
    }

    pub fn embed_query(&self, query: &str) -> Option<Vec<f32>> {
        match self {
            Engine::Small(model) => embed::embed_query(model, query),
            Engine::Large(server) => server
                .embed(&[gemma::query_prompt(query)])
                .map_err(|error| log::warn!("descriptive find: {error}"))
                .ok()?
                .pop(),
        }
    }

    /// Vectors for a file's passages, in order. The file's name goes with
    /// them: as the title of each for EmbeddingGemma (its document prompt),
    /// in front of the first for the small model. `None` if the model
    /// couldn't embed them; the passages stay findable by their words.
    pub fn embed_documents(&self, title: &str, texts: &[String]) -> Option<Vec<Vec<f32>>> {
        match self {
            Engine::Small(model) => {
                let mut inputs = texts.to_vec();
                if let Some(first) = inputs.first_mut() {
                    *first = format!("{title}\n{first}");
                }
                embed::embed_batch(model, &inputs)
            }
            Engine::Large(server) => {
                let mut vectors = Vec::with_capacity(texts.len());
                for batch in texts.chunks(GEMMA_BATCH) {
                    let prompts: Vec<String> = batch
                        .iter()
                        .map(|text| gemma::document_prompt(title, text))
                        .collect();
                    match server.embed(&prompts) {
                        Ok(batch) => vectors.extend(batch),
                        Err(error) => {
                            log::warn!("descriptive find: {error}");
                            return None;
                        }
                    }
                }
                Some(vectors)
            }
        }
    }
}
