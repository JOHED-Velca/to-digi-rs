use serde_json::{Map, Value};

use crate::error::AppError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigiwebStore {
    pub number: u32,
    pub name: String,
}

impl DigiwebStore {
    pub fn new(number: u32, name: impl Into<String>) -> Self {
        Self {
            number,
            name: name.into(),
        }
    }

    pub fn label(&self) -> String {
        format!("Store {} — {}", self.number, self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreResolution {
    Automatic(DigiwebStore),
    Configured(DigiwebStore),
    Explicit(DigiwebStore),
    ChoiceRequired { preferred_index: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegistryStorePage {
    pub stores: Vec<DigiwebStore>,
    pub record_count: usize,
}

pub fn resolve_store(
    stores: &[DigiwebStore],
    configured_number: u32,
    explicit_number: Option<u32>,
    interactive: bool,
) -> Result<StoreResolution, AppError> {
    if stores.is_empty() {
        return Err(AppError::DigiwebProcessing(
            "No active DIGIweb stores are configured. Add and activate a store before importing PLUs."
                .to_string(),
        ));
    }
    if let Some(number) = explicit_number {
        return stores
            .iter()
            .find(|store| store.number == number)
            .cloned()
            .map(StoreResolution::Explicit)
            .ok_or_else(|| {
                AppError::DigiwebProcessing(format!(
                    "Store {number} is no longer active. Run readiness again and select an active store."
                ))
            });
    }
    if stores.len() == 1 {
        return Ok(StoreResolution::Automatic(stores[0].clone()));
    }
    let configured_index = stores
        .iter()
        .position(|store| store.number == configured_number);
    if interactive {
        return Ok(StoreResolution::ChoiceRequired {
            preferred_index: configured_index.unwrap_or(0),
        });
    }
    configured_index
        .map(|index| StoreResolution::Configured(stores[index].clone()))
        .ok_or_else(|| {
            AppError::Config(format!(
                "DIGIweb has multiple active stores, but configured store {configured_number} is not one of them. Set digiweb.store_number to an active store or use the interactive TUI."
            ))
        })
}

pub(crate) fn parse_store_page(value: Value) -> Result<RegistryStorePage, AppError> {
    let records = records_from_response(&value).ok_or_else(|| {
        AppError::Http(
            "invalid DIGIweb store response: expected an array or paged store list".to_string(),
        )
    })?;
    let record_count = records.len();
    let mut stores = Vec::new();
    for record in records {
        let object = record.as_object().ok_or_else(|| {
            AppError::Http("invalid DIGIweb store response: store is not an object".to_string())
        })?;
        if !is_active(object) {
            continue;
        }
        let number = store_number(object).ok_or_else(|| {
            AppError::Http(
                "invalid DIGIweb store response: active store has no reference number".to_string(),
            )
        })?;
        if number == 0 {
            return Err(AppError::Http(
                "invalid DIGIweb store response: active store reference number is zero".to_string(),
            ));
        }
        let name = string_field(
            object,
            &[
                "name",
                "shopName",
                "shopname",
                "shop_name",
                "storeName",
                "storename",
                "store_name",
            ],
        )
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| format!("Store {number}"));
        stores.push(DigiwebStore::new(number, name));
    }
    stores.sort_by_key(|store| store.number);
    if stores
        .windows(2)
        .any(|pair| pair[0].number == pair[1].number)
    {
        return Err(AppError::Http(
            "invalid DIGIweb store response: duplicate active store reference number".to_string(),
        ));
    }
    Ok(RegistryStorePage {
        stores,
        record_count,
    })
}

#[cfg(test)]
fn parse_active_stores(value: Value) -> Result<Vec<DigiwebStore>, AppError> {
    parse_store_page(value).map(|page| page.stores)
}

fn records_from_response(value: &Value) -> Option<&Vec<Value>> {
    if let Value::Array(records) = value {
        return Some(records);
    }
    let object = value.as_object()?;
    for key in ["content", "items", "results", "rows", "shops", "stores"] {
        if let Some(records) = object.get(key).and_then(Value::as_array) {
            return Some(records);
        }
    }
    object.get("data").and_then(records_from_response)
}

fn store_number(object: &Map<String, Value>) -> Option<u32> {
    [
        "shopno",
        "shopNo",
        "shopNr",
        "shop_nr",
        "shopNumber",
        "shop_number",
        "storeNo",
        "storeno",
        "storeNr",
        "store_nr",
        "refNo",
        "referenceNo",
        "referenceNumber",
        "reference_number",
        "refno",
        "number",
        "no",
        "nr",
    ]
    .into_iter()
    .find_map(|key| object.get(key).and_then(value_as_u32))
}

fn value_as_u32(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .or_else(|| value.as_str()?.trim().parse().ok())
}

fn string_field(object: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| object.get(*key)?.as_str().map(ToString::to_string))
}

fn is_active(object: &Map<String, Value>) -> bool {
    if ["deleted", "isDeleted", "is_deleted"]
        .into_iter()
        .any(|key| object.get(key).and_then(value_as_bool) == Some(true))
    {
        return false;
    }
    for key in ["active", "isActive", "is_active", "enabled", "isEnabled"] {
        if let Some(active) = object.get(key).and_then(value_as_bool) {
            return active;
        }
    }
    !string_field(object, &["status", "state"]).is_some_and(|status| {
        matches!(
            status.trim().to_ascii_uppercase().as_str(),
            "INACTIVE" | "DEACTIVATED" | "DISABLED" | "DELETED" | "ARCHIVED"
        )
    })
}

fn value_as_bool(value: &Value) -> Option<bool> {
    value.as_bool().or_else(|| match value {
        Value::Number(number) => number.as_u64().map(|number| number != 0),
        Value::String(value) if value.eq_ignore_ascii_case("true") || value == "1" => Some(true),
        Value::String(value) if value.eq_ignore_ascii_case("false") || value == "0" => Some(false),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_active_shops_from_registry_page() {
        let body = serde_json::json!({
            "content": [
                {"shopno": 2, "name": "Starsky", "active": true},
                {"shopNo": 3, "shopName": "Closed", "active": false},
                {"storeNo": 8, "storeName": "Third Location", "status": "ACTIVE"}
            ]
        });

        assert_eq!(
            parse_active_stores(body).expect("stores"),
            vec![
                DigiwebStore::new(2, "Starsky"),
                DigiwebStore::new(8, "Third Location"),
            ]
        );
    }

    #[test]
    fn one_store_is_selected_even_when_configured_number_is_stale() {
        let stores = vec![DigiwebStore::new(2, "Starsky")];

        assert_eq!(
            resolve_store(&stores, 1, None, false).expect("selection"),
            StoreResolution::Automatic(stores[0].clone())
        );
    }

    #[test]
    fn interactive_multiple_stores_require_an_explicit_choice() {
        let stores = vec![
            DigiwebStore::new(2, "Starsky"),
            DigiwebStore::new(8, "Third Location"),
        ];

        assert_eq!(
            resolve_store(&stores, 2, None, true).expect("selection"),
            StoreResolution::ChoiceRequired { preferred_index: 0 }
        );
    }

    #[test]
    fn noninteractive_multiple_stores_use_only_a_valid_configured_number() {
        let stores = vec![
            DigiwebStore::new(2, "Starsky"),
            DigiwebStore::new(8, "Third Location"),
        ];

        assert_eq!(
            resolve_store(&stores, 8, None, false).expect("selection"),
            StoreResolution::Configured(stores[1].clone())
        );
        assert!(
            resolve_store(&stores, 1, None, false)
                .expect_err("stale config must be rejected")
                .to_string()
                .contains("multiple active stores")
        );
    }

    #[test]
    fn no_active_stores_blocks_import() {
        assert!(
            resolve_store(&[], 1, None, false)
                .expect_err("empty store list must fail")
                .to_string()
                .contains("No active DIGIweb stores")
        );
    }

    #[test]
    fn explicit_selection_must_still_be_active() {
        let stores = vec![DigiwebStore::new(2, "Starsky")];

        assert_eq!(
            resolve_store(&stores, 1, Some(2), true).expect("selection"),
            StoreResolution::Explicit(stores[0].clone())
        );
        assert!(
            resolve_store(&stores, 1, Some(3), true)
                .expect_err("missing explicit store must fail")
                .to_string()
                .contains("Store 3 is no longer active")
        );
    }
}
