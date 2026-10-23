p='crates/pi-session/src/agent_session/system_prompt.rs'
s=open(p).read()
# normalize: the reads exist for the throw effect (Value reads; raw-keeps
# re-read `input` for present-null vs missing).
s=s.replace('''	let custom_prompt = prop_or_err(input, "customPrompt")?;
	let force_system_prompt = prop_or_err(input, "forceSystemPrompt")?;
''','''	let _custom_prompt = prop_or_err(input, "customPrompt")?;
	let _force_system_prompt = prop_or_err(input, "forceSystemPrompt")?;
''')
s=s.replace('''	let cwd = prop_or_err(input, "cwd")?;
''','''	let _cwd = prop_or_err(input, "cwd")?;
''')
s=s.replace('''	let fsp = prop_or_err(input, "forceSystemPrompt")?;
''','''	let _fsp = prop_or_err(input, "forceSystemPrompt")?;
''')
s=s.replace('''	let mut add_rule = |rule: &Value''','''	let add_rule = |rule: &Value''')
open(p,'w').write(s)
print("fix11")
