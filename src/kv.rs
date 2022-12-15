use std::collections::HashMap;

use async_nats::{jetstream::kv::Store, Client};
use wasmbus_rpc::core::LinkDefinition;

use crate::GetClaimsResponse;
use crate::LinkDefinitionList;
use crate::Result;

pub(crate) async fn get_kv_store(
    nc: Client,
    lattice_prefix: &str,
    js_domain: Option<String>,
) -> Option<Store> {
    let jetstream = if let Some(domain) = js_domain {
        async_nats::jetstream::with_domain(nc, domain)
    } else {
        async_nats::jetstream::new(nc)
    };
    let bucket = format!("LATTICEDATA_{}", lattice_prefix);
    jetstream.get_key_value(bucket).await.ok()
}

pub(crate) async fn get_claims(store: &Store) -> Result<GetClaimsResponse> {
    let mut claims = Vec::new();
    let mut entries = store.keys().await?;
    while let Some(key) = entries.next() {
        if key.starts_with("CLAIMS_") {
            add_claim(&mut claims, store.get(key).await?).await?;
        }
    }
    Ok(GetClaimsResponse { claims: claims })
}

pub(crate) async fn get_links(store: &Store) -> Result<LinkDefinitionList> {
    let mut links = Vec::new();
    let mut entries = store.keys().await?;
    while let Some(key) = entries.next() {
        if key.starts_with("LINKDEF_") {
            add_linkdef(&mut links, store.get(key).await?).await?;
        }
    }

    Ok(LinkDefinitionList { links })
}

pub(crate) async fn put_link(store: &Store, ld: LinkDefinition) -> Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    let key = format!("LINKDEF_{}", id);
    store
        .put(key, serde_json::to_vec(&ld)?.into())
        .await
        .map(|_| ())
}

pub(crate) async fn delete_link(
    store: &Store,
    actor_id: &str,
    contract_id: &str,
    link_name: &str,
) -> Result<()> {
    if let Some(key) = find_link_key(store, actor_id, contract_id, link_name).await? {
        store.delete(key).await.map(|_| ())
    } else {
        Err("No such link".into())
    }
}

async fn add_linkdef(links: &mut Vec<LinkDefinition>, data: Option<Vec<u8>>) -> Result<()> {
    if let Some(d) = data {
        let ld: LinkDefinition = serde_json::from_slice(&d)?;
        links.push(ld);
    }

    Ok(())
}

async fn find_link_key(
    store: &Store,
    actor_id: &str,
    contract_id: &str,
    link_name: &str,
) -> Result<Option<String>> {
    let mut entries = store.keys().await?;
    while let Some(ref key) = entries.next() {
        if key.starts_with("LINKDEF_") {
            if let Some(raw) = store.get(key).await? {
                let ld: LinkDefinition = serde_json::from_slice(&raw)?;
                if ld.actor_id == actor_id
                    && ld.contract_id == contract_id
                    && ld.link_name == link_name
                {
                    return Ok(Some(key.to_string()));
                }
            }
        }
    }
    Ok(None)
}

async fn add_claim(claims: &mut Vec<HashMap<String, String>>, data: Option<Vec<u8>>) -> Result<()> {
    if let Some(d) = data {
        let json: HashMap<String, String> = serde_json::from_slice(&d)?;
        claims.push(json);
    }

    Ok(())
}

// NOTE: these tests require nats to be running with JS enabled. To run these, explicitly run them as they
// are difficult to run from within CI
#[cfg(test)]
mod test {
    use wasmbus_rpc::core::LinkDefinition;

    use crate::kv::{delete_link, get_claims, get_kv_store, get_links, put_link};

    const CLAIMS_1: &str = r#"{"call_alias":"","caps":"wasmcloud:httpserver","iss":"ABRIBHH54GM7QIEJBYYGZJUSDAMO34YM4SKWUQJGIILRB7JYGXEPWUVT","name":"kvcounter","rev":"1631624220","sub":"MBW3UGAIONCX3RIDDUGDCQIRGBQQOWS643CVICQ5EZ7SWNQPZLZTSQKU","tags":"","version":"0.3.0"}"#;
    const CLAIMS_2: &str = r#"{"call_alias":"","caps":"","iss":"ACOJJN6WUP4ODD75XEBKKTCCUJJCY5ZKQ56XVKYK4BEJWGVAOOQHZMCW","name":"HTTP Server","rev":"1644594344","sub":"VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M","tags":"","version":"0.14.10"}"#;

    const LINK_1: &str = r#"{"actor_id":"MBW3UGAIONCX3RIDDUGDCQIRGBQQOWS643CVICQ5EZ7SWNQPZLZTSQKU","contract_id":"wasmcloud:httpserver","id":"fb30deff-bbe7-4a28-a525-e53ebd4e8228","link_name":"default","provider_id":"VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M","values":{"PORT":"8082"}}"#;
    const LINK_2: &str = r#"{"actor_id":"MBW3UGAIONCX3RIDDUGDCQIRGBQQOWS643CVICQ5EZ7SWNQPZLZTSQKU","contract_id":"wasmcloud:keyvalue","id":"ff140106-dd0d-44ee-8241-a2158a528b1d","link_name":"default","provider_id":"VAZVC4RX54J2NVCMCW7BPCAHGGG5XZXDBXFUMDUXGESTMQEJLC3YVZWB","values":{"URL":"redis://127.0.0.1:6379"}}"#;

    #[tokio::test]
    async fn test_get_returns_none_for_nonexistent_store() {
        let client = async_nats::connect("127.0.0.1:4222").await.unwrap();

        let store = get_kv_store(client, "this-lattice-shall-never-existeth", None).await;
        assert!(store.is_none())
    }

    #[tokio::test]
    async fn test_get_claims_returns_response() {
        let client = async_nats::connect("127.0.0.1:4222").await.unwrap();
        let js = async_nats::jetstream::new(client.clone());
        let kv = js
            .create_key_value(async_nats::jetstream::kv::Config {
                bucket: "LATTICEDATA_mylattice1".to_string(),
                ..Default::default()
            })
            .await
            .unwrap();

        kv.put(
            "CLAIMS_VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M",
            CLAIMS_2.into(),
        )
        .await
        .unwrap();
        kv.put(
            "CLAIMS_MBW3UGAIONCX3RIDDUGDCQIRGBQQOWS643CVICQ5EZ7SWNQPZLZTSQKU",
            CLAIMS_1.into(),
        )
        .await
        .unwrap();

        let store = get_kv_store(client, "mylattice1", None).await.unwrap();
        let claims = get_claims(&store).await.unwrap();

        js.delete_key_value("LATTICEDATA_mylattice1".to_string())
            .await
            .unwrap();

        assert_eq!(claims.claims.len(), 2);
        assert!(claims.claims[0].contains_key("name"));
        assert!(claims.claims[0].contains_key("rev"));
        assert!(claims.claims[0].contains_key("sub"));
        assert!(claims.claims[1].contains_key("call_alias"));
    }

    #[tokio::test]
    async fn test_get_links_returns_response() {
        let client = async_nats::connect("127.0.0.1:4222").await.unwrap();
        let js = async_nats::jetstream::new(client.clone());
        let kv = js
            .create_key_value(async_nats::jetstream::kv::Config {
                bucket: "LATTICEDATA_mylattice2".to_string(),
                ..Default::default()
            })
            .await
            .unwrap();

        kv.put(
            "LINKDEF_ff140106-dd0d-44ee-8241-a2158a528b1d",
            LINK_2.into(),
        )
        .await
        .unwrap();
        kv.put("LINKDEF_fb30deff-bbe7-4a28-a525-e53ebd4e822", LINK_1.into())
            .await
            .unwrap();

        let store = get_kv_store(client, "mylattice2", None).await.unwrap();
        let links = get_links(&store).await.unwrap();

        js.delete_key_value("LATTICEDATA_mylattice2".to_string())
            .await
            .unwrap();

        assert_eq!(links.links.len(), 2);
    }

    #[tokio::test]
    async fn test_put_and_del_link() {
        let client = async_nats::connect("127.0.0.1:4222").await.unwrap();
        let js = async_nats::jetstream::new(client.clone());
        let kv = js
            .create_key_value(async_nats::jetstream::kv::Config {
                bucket: "LATTICEDATA_mylattice3".to_string(),
                ..Default::default()
            })
            .await
            .unwrap();

        let mut ld = LinkDefinition::default();
        ld.actor_id = "Mbob".to_string();
        ld.provider_id = "Valice".to_string();
        ld.contract_id = "wasmcloud:testy".to_string();
        ld.link_name = "default".to_string();
        put_link(&kv, ld).await.unwrap();

        let mut ld2 = LinkDefinition::default();
        ld2.actor_id = "Msteve".to_string();
        ld2.provider_id = "Valice".to_string();
        ld2.contract_id = "wasmcloud:testy".to_string();
        ld2.link_name = "default".to_string();
        put_link(&kv, ld2).await.unwrap();

        delete_link(&kv, "Mbob", "wasmcloud:testy", "default")
            .await
            .unwrap();

        let links = get_links(&kv).await.unwrap();

        js.delete_key_value("LATTICEDATA_mylattice3".to_string())
            .await
            .unwrap();

        assert_eq!(links.links.len(), 1); // 1 left after delete
    }
}
