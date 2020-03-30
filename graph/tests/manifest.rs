use async_trait::async_trait;
use slog::Logger;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use graph::components::link_resolver::{JsonValueStream, LinkResolver as LinkResolverTrait};
use graph::prelude::{
    Entity, Link, SubgraphDeploymentId, SubgraphManifest, SubgraphManifestValidationError,
    UnvalidatedSubgraphManifest,
};

use test_store::LOGGER;

#[derive(Default)]
struct TextResolver {
    texts: HashMap<String, String>,
}

impl TextResolver {
    fn add(&mut self, link: &str, text: &str) {
        self.texts.insert(link.to_owned(), text.to_owned());
    }
}

#[async_trait]
impl LinkResolverTrait for TextResolver {
    fn with_timeout(self, _timeout: Duration) -> Self {
        self
    }

    fn with_retries(self) -> Self {
        self
    }

    async fn cat(&self, _logger: &Logger, link: &Link) -> Result<Vec<u8>, failure::Error> {
        self.texts
            .get(&link.link)
            .ok_or(failure::format_err!("No text for {}", &link.link))
            .map(|text| text.to_owned().into_bytes())
    }

    async fn json_stream(
        &self,
        _logger: &Logger,
        _link: &Link,
    ) -> Result<JsonValueStream, failure::Error> {
        unimplemented!()
    }
}

const GQL_SCHEMA: &str = "type Thing @entity { id: ID! }";

async fn resolve_manifest(text: &str) -> SubgraphManifest {
    let mut resolver = TextResolver::default();
    let link = Link::from("/ipfs/Qmmanifest".to_owned());

    resolver.add(link.link.as_str(), text);
    resolver.add("/ipfs/Qmschema", GQL_SCHEMA);

    SubgraphManifest::resolve(link, &resolver, &LOGGER)
        .await
        .expect("Parsing simple manifest works")
}

async fn resolve_unvalidated(text: &str) -> UnvalidatedSubgraphManifest {
    let mut resolver = TextResolver::default();
    let link = Link::from("/ipfs/Qmmanifest".to_owned());

    resolver.add(link.link.as_str(), text);
    resolver.add("/ipfs/Qmschema", GQL_SCHEMA);

    UnvalidatedSubgraphManifest::resolve(link, Arc::new(resolver), &LOGGER)
        .await
        .expect("Parsing simple manifest works")
}

#[tokio::test]
async fn simple_manifest() {
    const YAML: &str = "
dataSources: []
schema:
  file:
    /: /ipfs/Qmschema
specVersion: 0.0.1
";

    let manifest = resolve_manifest(YAML).await;

    assert_eq!("Qmmanifest", manifest.id.as_str());
    assert!(manifest.graft.is_none());
}

#[tokio::test]
async fn graft_manifest() {
    const YAML: &str = "
dataSources: []
schema:
  file:
    /: /ipfs/Qmschema
graft:
  base: Qmbase
  block: 12345
specVersion: 0.0.1
";

    let manifest = resolve_manifest(YAML).await;

    assert_eq!("Qmmanifest", manifest.id.as_str());
    let graft = manifest.graft.expect("The manifest has a graft base");
    assert_eq!("Qmbase", graft.base.as_str());
    assert_eq!(12345, graft.block);
}

#[test]
fn graft_invalid_manifest() {
    const YAML: &str = "
dataSources: []
schema:
  file:
    /: /ipfs/Qmschema
graft:
  base: Qmbase
  block: 1
specVersion: 0.0.1
";

    let store = test_store::STORE.clone();

    test_store::STORE_RUNTIME.lock().unwrap().block_on(async {
        let unvalidated = resolve_unvalidated(YAML).await;
        let subgraph = SubgraphDeploymentId::new("Qmbase").unwrap();

        //
        // Validation against subgraph that hasn't synced anything fails
        //
        test_store::create_test_subgraph(subgraph.as_str(), GQL_SCHEMA);
        // This check is awkward since the test manifest has other problems
        // that the validation complains about as setting up a valid manifest
        // would be a bit more work; we just want to make sure that
        // graft-related checks work
        let msg = unvalidated
            .validate(store.clone())
            .expect_err("Validation must fail")
            .into_iter()
            .find(|e| matches!(e, SubgraphManifestValidationError::GraftBaseInvalid(_)))
            .expect("There must be a GraftBaseInvalid error")
            .to_string();
        assert_eq!(
            "the graft base is invalid: can not graft onto `1` since \
            it has not processed any blocks",
            msg
        );

        let mut thing = Entity::new();
        thing.set("id", "datthing");
        test_store::insert_entities(subgraph, vec![("Thing", thing)]).expect("Can insert a thing");

        // Validation against subgraph that has not reached the graft point fails
        let unvalidated = resolve_unvalidated(YAML).await;
        let msg = unvalidated
            .validate(store)
            .expect_err("Validation must fail")
            .into_iter()
            .find(|e| matches!(e, SubgraphManifestValidationError::GraftBaseInvalid(_)))
            .expect("There must be a GraftBaseInvalid error")
            .to_string();
        assert_eq!(
            "the graft base is invalid: can not graft onto `Qmbase` \
            at block 1 since it has only processed block 0",
            msg
        );
    })
}
