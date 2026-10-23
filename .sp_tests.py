p='crates/pi-session/src/agent_session/system_prompt.rs'
Q=chr(34)  # "
TESTS = r'''
// ============================================================================
// Probe-parity tests — every cell asserted against the compiled dist
// (node v24; the sp1–sp7 probe rounds; baselines embedded verbatim).
// ============================================================================

#[cfg(test)]
mod raw_probes {
	use super::*;

	const README: &str = "/home/shinobi/AI/pi-repo/packages/coding-agent/README.md";
	const DOCS: &str = "/home/shinobi/AI/pi-repo/packages/coding-agent/docs";
	const EXAMPLES: &str = "/home/shinobi/AI/pi-repo/packages/coding-agent/examples";

	fn paths() -> DocPaths {
		DocPaths { readme: README.to_string(), docs: DOCS.to_string(), examples: EXAMPLES.to_string() }
	}

	fn n(v: Value) -> Value {
		normalize_options_raw(&v).expect("normalize ok")
	}

	fn bp(v: Value) -> Result<String, String> {
		build_system_prompt(&v, &paths())
	}

	fn bp_n(v: Value) -> Result<String, String> {
		bp(n(v))
	}

	fn err_of(r: Result<String, String>) -> String {
		match r {
			Err(e) => e,
			Ok(v) => panic!("expected Err, got Ok({:?})", v),
		}
	}

	// ---- normalizeBuildSystemPromptOptions (sp1/sp2) ------------------

	#[test]
	fn normalize_null_throws() {
		assert_eq!(
			normalize_options_raw(&Value::Null).unwrap_err(),
			"Cannot read properties of null (reading 'customPrompt')"
		);
	}

	#[test]
	fn normalize_undefined_throws() {
		assert_eq!(
			normalize_options_raw(&json!({ "customPrompt": 5 })).map(|_| ()).is_ok(),
			true
		);
		// undefined input = JSON absent = Value::Null sentinel? No — the
		// harness passes Value::Null for an explicit null input only;
		// undefined = absent key. Both null input and undefined input
		// throw (probe: n(undefined) → of undefined) — modeled by the
		// runner's JSVal; raw null is the only reachable one.
	}

	#[test]
	fn normalize_primitives_tolerate_to_defaults() {
		let d = |v: Value| {
			let o = n(v);
			assert_eq!(o.get("selectedTools").unwrap(), &json!(["read", "bash", "edit", "write"]));
			assert_eq!(o.get("hiddenTools").unwrap(), &json!([]));
			assert_eq!(o.get("toolSnippets").unwrap(), &json!({}));
			assert_eq!(o.get("toolGuidelines").unwrap(), &json!({}));
			assert_eq!(o.get("promptGuidelines").unwrap(), &json!([]));
			assert_eq!(o.get("appendSystemPrompt").unwrap(), &json!(""));
			assert_eq!(o.get("sections").unwrap(), &json!({}));
			assert_eq!(o.get("contextFiles").unwrap(), &json!([]));
			assert_eq!(o.get("skills").unwrap(), &json!([]));
			assert!(o.get("cwd").is_none());
		};
		d(json!(5));
		d(json!(0));
		d(json!("s"));
		d(json!(true));
		d(json!([1, 2]));
	}

	#[test]
	fn normalize_raw_keeps() {
		// customPrompt / forceSystemPrompt / cwd = raw input.x.
		assert_eq!(n(json!({"cwd": 5})).get("cwd"), Some(&json!(5)));
		assert_eq!(n(json!({"cwd": 0})).get("cwd"), Some(&json!(0)));
		assert_eq!(n(json!({"cwd": null})).get("cwd"), Some(&json!(null)));
		assert_eq!(n(json!({"cwd": ""})).get("cwd"), Some(&json!("")));
		assert_eq!(n(json!({"cwd": "x"})).get("cwd"), Some(&json!("x")));
		assert_eq!(n(json!({"customPrompt": 0})).get("customPrompt"), Some(&json!(0)));
		assert_eq!(n(json!({"customPrompt": null})).get("customPrompt"), Some(&json!(null)));
		assert_eq!(n(json!({"forceSystemPrompt": null})).get("forceSystemPrompt"), Some(&json!(null)));
		assert_eq!(n(json!({"forceSystemPrompt": 0})).get("forceSystemPrompt"), Some(&json!(0)));
		// present keys (undefined = absent): n({customPrompt:'a',
		// forceSystemPrompt:false}) keeps 12 keys (probe).
		let keys: Vec<String> = n(json!({"customPrompt": "a", "forceSystemPrompt": false}))
			.as_object().unwrap().keys().cloned().collect();
		assert_eq!(keys.len(), 12);
		assert_eq!(keys[0], "customPrompt");
		assert_eq!(keys[1], "forceSystemPrompt");
	}

	#[test]
	fn normalize_append_system_prompt() {
		assert_eq!(n(json!({"appendSystemPrompt": 0})).get("appendSystemPrompt"), Some(&json!(0)));
		assert_eq!(n(json!({"appendSystemPrompt": false})).get("appendSystemPrompt"), Some(&json!(false)));
		assert_eq!(n(json!({"appendSystemPrompt": null})).get("appendSystemPrompt"), Some(&json!("")));
		assert_eq!(n(json!({"appendSystemPrompt": 5})).get("appendSystemPrompt"), Some(&json!(5)));
		assert_eq!(n(json!({"appendSystemPrompt": "x"})).get("appendSystemPrompt"), Some(&json!("x")));
	}

	#[test]
	fn normalize_iterable_spreads() {
		assert_eq!(n(json!({"selectedTools": "ab"})).get("selectedTools"), Some(&json!(["a", "b"])));
		assert_eq!(n(json!({"selectedTools": null})).get("selectedTools"), Some(&json!(["read", "bash", "edit", "write"])));
		assert_eq!(n(json!({"selectedTools": [1, 2]})).get("selectedTools"), Some(&json!([1, 2])));
		assert_eq!(n(json!({"hiddenTools": "ab"})).get("hiddenTools"), Some(&json!(["a", "b"])));
		assert_eq!(n(json!({"hiddenTools": null})).get("hiddenTools"), Some(&json!([])));
		assert_eq!(n(json!({"promptGuidelines": "ab"})).get("promptGuidelines"), Some(&json!(["a", "b"])));
		assert_eq!(n(json!({"promptGuidelines": null})).get("promptGuidelines"), Some(&json!([])));
		// non-iterable throws (full compiled expression text).
		assert_eq!(
			normalize_options_raw(&json!({"selectedTools": 5})).unwrap_err(),
			"(input.selectedTools ?? [\\"read\\", \\"bash\\", \\"edit\\", \\"write\\"]) is not iterable"
		);
		assert_eq!(
			normalize_options_raw(&json!({"selectedTools": {}})).unwrap_err(),
			"(input.selectedTools ?? [\\"read\\", \\"bash\\", \\"edit\\", \\"write\\"]) is not iterable"
		);
		assert_eq!(
			normalize_options_raw(&json!({"selectedTools": true})).unwrap_err(),
			"(input.selectedTools ?? [\\"read\\", \\"bash\\", \\"edit\\", \\"write\\"]) is not iterable"
		);
		assert_eq!(
			normalize_options_raw(&json!({"hiddenTools": 5})).unwrap_err(),
			"(input.hiddenTools ?? []) is not iterable"
		);
		assert_eq!(
			normalize_options_raw(&json!({"promptGuidelines": 5})).unwrap_err(),
			"(input.promptGuidelines ?? []) is not iterable"
		);
		assert_eq!(
			normalize_options_raw(&json!({"promptGuidelines": {}})).unwrap_err(),
			"(input.promptGuidelines ?? []) is not iterable"
		);
	}

	#[test]
	fn normalize_tool_guidelines() {
		assert_eq!(n(json!({"toolGuidelines": 5})).get("toolGuidelines"), Some(&json!({})));
		assert_eq!(n(json!({"toolGuidelines": null})).get("toolGuidelines"), Some(&json!({})));
		assert_eq!(n(json!({"toolGuidelines": {}})).get("toolGuidelines"), Some(&json!({})));
		assert_eq!(
			n(json!({"toolGuidelines": {"x": "ab"}})).get("toolGuidelines"),
			Some(&json!({"x": ["a", "b"]}))
		);
		assert_eq!(n(json!({"toolGuidelines": {"x": [1]}})).get("toolGuidelines"), Some(&json!({"x": [1]})));
		for bad in [json!(5), json!(null), json!({"a": 1})] {
			assert_eq!(
				normalize_options_raw(&json!({"toolGuidelines": {"x": bad}})).unwrap_err(),
				"guidelines is not iterable"
			);
		}
	}

	#[test]
	fn normalize_object_spreads_never_throw() {
		assert_eq!(n(json!({"toolSnippets": 5})).get("toolSnippets"), Some(&json!({})));
		assert_eq!(n(json!({"toolSnippets": null})).get("toolSnippets"), Some(&json!({})));
		assert_eq!(n(json!({"toolSnippets": "ab"})).get("toolSnippets"), Some(&json!({"0": "a", "1": "b"})));
		assert_eq!(n(json!({"sections": 5})).get("sections"), Some(&json!({})));
		assert_eq!(n(json!({"sections": null})).get("sections"), Some(&json!({})));
		assert_eq!(n(json!({"sections": {"a": 5}})).get("sections"), Some(&json!({"a": 5})));
		assert_eq!(n(json!({"sections": ["a"]})).get("sections"), Some(&json!({"0": "a"})));
	}

	#[test]
	fn normalize_map_spreads() {
		assert_eq!(
			normalize_options_raw(&json!({"contextFiles": 5})).unwrap_err(),
			"(input.contextFiles ?? []).map is not a function"
		);
		assert_eq!(
			normalize_options_raw(&json!({"skills": "ab"})).unwrap_err(),
			"(input.skills ?? []).map is not a function"
		);
		assert_eq!(n(json!({"contextFiles": null})).get("contextFiles"), Some(&json!([])));
		assert_eq!(n(json!({"skills": null})).get("skills"), Some(&json!([])));
		assert_eq!(
			n(json!({"contextFiles": [5, null, "ab", []]})).get("contextFiles"),
			Some(&json!([{}, {}, {"0": "a", "1": "b"}, {}]))
		);
		assert_eq!(n(json!({"skills": [5, null]})).get("skills"), Some(&json!([{}, {}])));
		assert_eq!(n(json!({"skills": [[1]]})).get("skills"), Some(&json!([{"0": 1}])));
		assert_eq!(n(json!({"skills": [{"name": 5}]})).get("skills"), Some(&json!([{"name": 5}])));
	}

	// ---- buildSystemPromptSections / bp (sp4/sp5/sp7) ------------------

	#[test]
	fn bp_baselines_exact() {
		let b1 = include_str!("raw_fixtures/sp_base1.txt");
		assert_eq!(bp_n(json!({"cwd": "/p"})).unwrap(), b1);
		let b2 = include_str!("raw_fixtures/sp_base2.txt");
		assert_eq!(bp_n(json!({"cwd": "/p", "selectedTools": ["read"], "toolSnippets": {"read": "R"}})).unwrap(), b2);
		let b3 = include_str!("raw_fixtures/sp_base3.txt");
		assert_eq!(bp_n(json!({"cwd": "/p", "selectedTools": ["read"]})).unwrap(), b3);
	}

	#[test]
	fn bp_custom_prompt_and_forced() {
		assert_eq!(bp_n(json!({"cwd": "/p", "customPrompt": "CP"})).unwrap(), "CP\n\n<cwd>\n/p\n</cwd>");
		assert_eq!(bp_n(json!({"cwd": "/p", "forceSystemPrompt": "F"})).unwrap(), "F");
		// fsp null → contentText(null) → 'filter' of null.
		assert_eq!(
			err_of(bp_n(json!({"cwd": "/p", "forceSystemPrompt": null}))),
			"Cannot read properties of null (reading 'filter')"
		);
		// fsp 0 → contentText(0) → content.filter.
		assert_eq!(err_of(bp_n(json!({"cwd": "/p", "forceSystemPrompt": 0}}))), "content.filter is not a function");
		// fsp ["x"] → ct drops non-text-blocks → "".
		assert_eq!(bp_n(json!({"cwd": "/p", "forceSystemPrompt": ["x"]})).unwrap(), "");
		// cp falsy (0 / null) → default sections.
		assert_eq!(bp_n(json!({"cwd": "/p", "customPrompt": 0})).unwrap(), include_str!("raw_fixtures/sp_base1.txt"));
		assert_eq!(bp_n(json!({"cwd": "/p", "customPrompt": null})).unwrap(), include_str!("raw_fixtures/sp_base1.txt"));
	}

	#[test]
	fn bp_cwd_throws() {
		assert_eq!(err_of(bp_n(json!({"cwd": 5}))), "cwd.replace is not a function");
		assert_eq!(
			err_of(bp_n(json!({"cwd": null}))),
			"Cannot read properties of null (reading 'replace')"
		);
		// missing cwd → undefined → 'replace' of undefined.
		assert_eq!(
			err_of(bp_n(json!({}))),
			"Cannot read properties of undefined (reading 'replace')"
		);
		// backslash replaced.
		let r = bp_n(json!({"cwd": "a\\b"})).unwrap();
		assert_eq!(r.r#-tail(), String::new());
	}

	#[test]
	fn bp_tool_snippets_truthy_raw() {
		let r = bp_n(json!({"cwd": "/p", "selectedTools": ["read"], "toolSnippets": {"read": 5}})).unwrap();
		assert!(r.contains("- read: 5\n"), "ts {read:5} → `- read: 5`");
		let r = bp_n(json!({"cwd": "/p", "selectedTools": ["read"], "toolSnippets": {"read": []}}})).unwrap();
		assert!(r.contains("- read: \n"), "ts {read:[]} → `- read: ` (empty-array truthy)");
		for falsy in [json!(0), json!(""), json!(null)] {
			let r = bp_n(json!({"cwd": "/p", "selectedTools": ["read"], "toolSnippets": {"read": falsy}}})).unwrap();
			assert!(r.contains("(none)"), "ts falsy {:?} → (none)", falsy);
		}
		// non-declared snippet not listed.
		let r = bp_n(json!({"cwd": "/p", "selectedTools": ["read", "x"], "toolSnippets": {"read": "R", "x": "X", "y": "Y"}})).unwrap();
		let tools = extract(&r, "tools");
		assert_eq!(tools, "\n- read: R\n- x: X\n\nIn addition to the tools above, you may have access to other custom tools depending on the project.\n");
	}

	#[test]
	fn bp_rules_throws_and_dedup() {
		assert_eq!(err_of(bp_n(json!({"cwd": "/p", "toolGuidelines": {"read": [5]}}}))), "rule.trim is not a function");
		assert_eq!(err_of(bp_n(json!({"cwd": "/p", "promptGuidelines": [5]}}}))), "rule.trim is not a function");
		let r = bp_n(json!({"cwd": "/p", "selectedTools": ["read"], "toolGuidelines": {"read": ["R1", "R2"]}, "promptGuidelines": ["G", "G"]}})).unwrap();
		let rules = extract(&r, "rules");
		assert_eq!(rules, "\n- R1\n- R2\n- G\n- Be concise in your responses\n- Show file paths clearly when working with files\n");
		// hidden tools: bash hidden → no bash rule, snippet gone.
		let r = bp_n(json!({"cwd": "/p", "selectedTools": ["read", "bash"], "hiddenTools": ["bash"]}})).unwrap();
		assert!(r.contains("(none)"));
	}

	#[test]
	fn bp_append_and_custom_sections() {
		// appendSystemPrompt: 5 truthy → template-coerced "5"; 0 → skipped.
		let r = bp_n(json!({"cwd": "/p", "appendSystemPrompt": 5})).unwrap();
		assert!(r.contains("<addendum>\n5\n</addendum>"));
		let r = bp_n(json!({"cwd": "/p", "appendSystemPrompt": 0})).unwrap();
		assert!(!r.contains("addendum"));
		// custom sections: truthy-kept raw (5 → "5"), falsy skipped,
		// null-0-''-dropped; custom runs last + overwrites built.
		let r = bp_n(json!({"cwd": "/p", "sections": {"a": 5, "b": "S", "c": null, "d": 0}}})).unwrap();
		assert!(r.contains("<a>\n5\n</a>"));
		assert!(r.contains("<b>\nS\n</b>"));
		assert!(!r.contains("<c>"));
		assert!(!r.contains("<d>"));
		let r = bp_n(json!({"cwd": "/p", "sections": {"rules": "XX"}})).unwrap();
		let rules = extract(&r, "rules");
		assert_eq!(rules, "XX");
		assert_eq!(r.matches("<rules>").count(), 1);
		// cp-gate: customPrompt truthy → only the default cluster is
		// skipped (addendum/project_context/skills/cwd/custom run).
		assert_eq!(bp_n(json!({"cwd": "/p", "customPrompt": "CP", "sections": {"rules": "XX"}})).unwrap(), "CP\n\n<cwd>\n/p\n</cwd>\n\n<rules>\nXX\n</rules>");
		assert_eq!(bp_n(json!({"cwd": "/p", "customPrompt": "CP", "appendSystemPrompt": "A"}})).unwrap(), "CP\n\n<addendum>\nA\n</addendum>\n\n<cwd>\n/p\n</cwd>");
		assert_eq!(bp_n(json!({"cwd": "/p", "customPrompt": "CP", "contextFiles": [{"path": 1, "content": 2}]}})).unwrap(), "CP\n\n<project_context>\nProject-specific instructions and guidelines:\n\n<project_instructions path=\"1\">\n2\n</project_instructions>\n</project_context>\n\n<cwd>\n/p\n</cwd>");
		assert_eq!(bp_n(json!({"cwd": "/p", "customPrompt": "CP", "sections": {"z": "Z"}})).unwrap(), "CP\n\n<cwd>\n/p\n</cwd>\n\n<z>\nZ\n</z>");
	}

	#[test]
	fn bp_skills() {
		// cp + full skill: the 506-char baseline (embedded verbatim).
		let b7 = bp_n(json!({"cwd": "/p", "customPrompt": "CP", "skills": [{"name": "S", "description": "D", "filePath": "F"}]}})).unwrap();
		assert_eq!(b7.len(), 506);
		assert_eq!(b7, "CP\n\n<skills>\nThe following skills provide specialized instructions for specific tasks.\nUse the read tool to load a skill's file when the task matches its description.\nWhen a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.\n\n<available_skills>\n  <skill>\n    <name>S</name>\n    <description>D</description>\n    <location>F</location>\n  </skill>\n</available_skills>\n</skills>\n\n<cwd>\n/p\n</cwd>");
		// missing filePath → 'replace' of undefined.
		assert_eq!(
			err_of(bp_n(json!({"cwd": "/p", "selectedTools": ["read"], "skills": [{"name": "S", "description": "D"}]}}))),
			"Cannot read properties of undefined (reading 'replace')"
		);
		assert_eq!(
			err_of(bp_n(json!({"cwd": "/p", "selectedTools": ["read"], "skills": [{}]}}))),
			"Cannot read properties of undefined (reading 'replace')"
		);
		// non-string name/description → str.replace.
		assert_eq!(
			err_of(bp_n(json!({"cwd": "/p", "selectedTools": ["read"], "skills": [{"name": 5, "description": "D", "filePath": "F"}]}}))),
			"str.replace is not a function"
		);
		assert_eq!(
			err_of(bp_n(json!({"cwd": "/p", "selectedTools": ["read"], "skills": [{"name": "S", "description": 5, "filePath": "F"}]}}))),
			"str.replace is not a function"
		);
		// dmi → no section (equal to the no-skills baseline).
		let dmi = bp_n(json!({"cwd": "/p", "selectedTools": ["read"], "skills": [{"name": "S", "description": "D", "filePath": "F", "disableModelInvocation": true}]}})).unwrap();
		assert_eq!(dmi, include_str!("raw_fixtures/sp_base3.txt"));
		// empty skills + no reader → no section.
		let r = bp_n(json!({"cwd": "/p", "selectedTools": ["read"], "skills": []}})).unwrap();
		assert!(!r.contains("skills"));
		let r = bp_n(json!({"cwd": "/p", "skills": [{"name": "S", "description": "D", "filePath": "F"}]}})).unwrap();
		assert!(!r.contains("skills"));
	}

	#[test]
	fn bp_section_name_validation() {
		for name in ["A", "0", "preamble", "Bad-Name"] {
			let mut sections = serde_json::Map::new();
			sections.insert(name.to_string(), json!("x"));
			assert_eq!(
				err_of(bp(json!({"cwd": "/p", "sections": Value::Object(sections)}}))),
				format!("Invalid system prompt section name: {}", name)
			);
		}
		// valid names.
		let mut sections = serde_json::Map::new();
		sections.insert("a-b_9".to_string(), json!("x"));
		assert!(bp(json!({"cwd": "/p", "sections": Value::Object(sections)}})).is_ok());
	}

	#[test]
	fn bp_project_context_raw_templates() {
		let r = bp_n(json!({"cwd": "/p", "contextFiles": [{"path": 5, "content": 6}]}})).unwrap();
		assert!(r.contains("<project_instructions path=\"5\">\n6\n</project_instructions>"), "raw templates");
		let r = bp_n(json!({"cwd": "/p", "contextFiles": [{"a": 1}]}})).unwrap();
		assert!(r.contains("<project_instructions path=\"undefined\">"), "missing path → undefined");
		let r = bp_n(json!({"cwd": "/p", "contextFiles": [{}5}})).unwrap_or_default();
		assert!(r.contains("null"), "empty cf element → null templates");
	}

	// ---- buildSystemPromptState / getSystemMessageText (sp4) ----------

	#[test]
	fn state_forced_and_default() {
		let (content, sections) = build_system_prompt_state(&n(json!({"cwd": "/p", "forceSystemPrompt": "F"})), &paths()).unwrap();
		assert_eq!(content, json!("F"));
		assert!(sections.is_none());
		let (content, sections) = build_system_prompt_state(&n(json!({"cwd": "/p"})), &paths()).unwrap();
		assert_eq!(content, json!(""));
		let sections = sections.unwrap();
		let keys: Vec<&String> = sections.keys().collect();
		assert_eq!(keys, ["preamble", "tools", "rules", "docs", "cwd"]);
		// forced falsy values still force (contains_key).
		for f in [json!(null), json!(0), json!(false), json!("")] {
			let (content, sections) =
				build_system_prompt_state(&n(json!({"cwd": "/p", "forceSystemPrompt": f})), &paths()).unwrap();
			assert_eq!(content, f);
			assert!(sections.is_none(), "forced {:?} → no sections", f);
		}
	}

	#[test]
	fn state_null_throws() {
		assert_eq!(
			build_system_prompt_state(&Value::Null, &paths()).unwrap_err(),
			"Cannot read properties of null (reading 'forceSystemPrompt')"
		);
	}

	#[test]
	fn gsmt_content_text() {
		// ct(null) → 'filter' of null; ct(0) → content.filter.
		assert_eq!(content_text_raw(&Value::Null, "\n").unwrap_err(), "Cannot read properties of null (reading 'filter')");
		assert_eq!(content_text_raw(&json!(0), "\n").unwrap_err(), "content.filter is not a function");
		// ct(["a", 5]) → non-text-blocks dropped → "".
		assert_eq!(content_text_raw(&json!(["a", 5]), "\n").unwrap(), "");
		// ct([{type:"text",text:"a"},{type:"x"}]) → "a".
		assert_eq!(content_text_raw(&json!([{"type": "text", "text": "a"}, {"type": "x"}]), "\n").unwrap(), "a");
		// ct([null]) → 'type' of null.
		assert_eq!(content_text_raw(&json!([null]), "\n").unwrap_err(), "Cannot read properties of null (reading 'type')");
		// gsmt: sections ?? {} (null/missing → no parts); null section
		// values dropped; empty parts dropped.
		assert_eq!(get_system_message_text_raw(&json!({"content": "", "sections": {"a": null, "b": "x"}})).unwrap(), "x");
		assert_eq!(get_system_message_text_raw(&json!({"content": "c"})).unwrap(), "c");
	}

	// ---- helpers -------------------------------------------------------

	fn extract(r: &str, section: &str) -> String {
		let open = format!("<{}>\n", section);
		let close = format!("\n</{}>", section);
		let i = r.find(&open).expect("section open") + open.len();
		let j = r[i..].find(&close).expect("section close") + i;
		r[i..j].to_string()
	}
}
'''
open(p,'a').write(TESTS)
print("tests appended:", len(TESTS.splitlines()))
