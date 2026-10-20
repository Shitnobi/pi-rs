//! Shared `macro_rules!` for order-preserving JSON object structs.
//!
//! Declared before `model`/`credential` in `mod.rs` so both can build
//! structs that round-trip TS interfaces losslessly (known typed fields +
//! `extra` map for unknown keys + recorded key order).


/// Field extraction for `js_object!` structs. `req` fields error with serde's
/// `missing field` wording when absent; `opt` fields (always `Option<_>`)
/// default to `None` (JSON `null` also maps to `None`).
macro_rules! field_take {
	(req, $map:expr, $key:literal) => {
		match $map.remove($key) {
			Some(v) => serde_json::from_value(v).map_err(|e| e.to_string())?,
			None => return Err(format!("missing field `{}`", $key)),
		}
	};
	(opt, $map:expr, $key:literal) => {
		match $map.remove($key) {
			Some(v) => serde_json::from_value(v).map_err(|e| e.to_string())?,
			None => None,
		}
	};
}

/// Field emission for `js_object!` structs. `opt` fields emit only when
/// `Some` (an absent/`None` field is dropped, like `JSON.stringify` drops
/// `undefined`).
macro_rules! field_emit {
	(req, $field:expr) => {
		Some(serde_json::to_value($field).unwrap_or(serde_json::Value::Null))
	};
	(opt, $field:expr) => {
		$field.as_ref().map(|v| serde_json::to_value(v).unwrap_or(serde_json::Value::Null))
	};
}

/// Declares a struct mirroring a TS interface with lossless, order-preserving
/// JSON round-trip. See the module docs. Field syntax:
/// `name kind "jsonKey" : Type` (`kind` is `req` or `opt`).
macro_rules! js_object {
	(
		$(#[$outer:meta])*
		pub struct $name:ident {
			$( $field:ident $kind:ident $key:literal : $ty:ty ),* $(,)?
		}
	) => {
		$(#[$outer])*
		#[derive(Debug, Clone, PartialEq)]
		pub struct $name {
			$( pub $field: $ty, )*
			/// Unknown fields, in original key order.
			pub extra: serde_json::Map<String, serde_json::Value>,
			/// Original key order of known + unknown fields.
			order: Vec<String>,
		}

		impl $name {
			/// Build from an ordered JSON object map. Known keys are extracted
			/// into typed fields, unknown keys go to `extra`, and the key order
			/// is recorded for faithful re-serialization.
			pub fn from_map(mut map: serde_json::Map<String, serde_json::Value>) -> Result<Self, String> {
				let order: Vec<String> = map.keys().cloned().collect();
				$( let $field = field_take!($kind, &mut map, $key); )*
				Ok(Self {
					$( $field, )*
					extra: map,
					order,
				})
			}

			/// TS `{...model, key: value}` — sets a known or unknown field.
			/// Existing keys keep their position; new keys are appended
			/// (exactly like a JS object assignment after a spread).
			pub fn set(&mut self, key: &str, value: Value) -> Result<(), String> {
				match key {
					$( $key => { self.$field = serde_json::from_value(value).map_err(|e| e.to_string())?; } )*
					_ => {
						self.extra.insert(key.to_string(), value);
					}
				}
				if !self.order.iter().any(|k| k == key) {
					self.order.push(key.to_string());
				}
				Ok(())
			}

			/// The ordered JSON object for this value (known + extra fields,
			/// original key order; newly-set fields appended).
			pub fn ordered_map(&self) -> serde_json::Map<String, serde_json::Value> {
				let mut out = Map::new();
				for key in &self.order {
					if let Some(v) = self.emit_known(key) {
						out.insert(key.clone(), v);
					} else if let Some(v) = self.extra.get(key) {
						out.insert(key.clone(), v.clone());
					}
					// else: known key whose field is now None → dropped
					// (JSON.stringify drops undefined).
				}
				$(
					if !self.order.iter().any(|k| k == $key) {
						if let Some(v) = field_emit!($kind, &self.$field) {
							out.insert($key.to_string(), v);
						}
					}
				)*
				for (k, v) in &self.extra {
					if !self.order.iter().any(|k2| k2 == k) {
						out.insert(k.clone(), v.clone());
					}
				}
				out
			}

			pub fn to_value(&self) -> Value {
				serde_json::Value::Object(self.ordered_map())
			}

			fn emit_known(&self, key: &str) -> Option<Value> {
				match key {
					$( $key => field_emit!($kind, &self.$field), )*
					_ => None,
				}
			}
		}

		impl serde::Serialize for $name {
			fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
				self.ordered_map().serialize(serializer)
			}
		}

		impl<'de> serde::Deserialize<'de> for $name {
			fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
				let map = Map::<String, Value>::deserialize(deserializer)?;
				Self::from_map(map).map_err(serde::de::Error::custom)
			}
		}
	};
}
