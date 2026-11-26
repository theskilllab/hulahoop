-- Harnesses report the model without its provider (`qwen3.8-flash` for
-- `opencode-go/qwen3.8-flash`), so one model was counted under two names in
-- Cost control. The reported name now carries the provider of the configured
-- model; Claude Code only runs Anthropic models.
update runs
set actual_model = substr(model, 1, instr(model, '/')) || actual_model
where actual_model is not null
  and actual_model != 'mixed'
  and instr(actual_model, '/') = 0
  and instr(model, '/') > 0;

update runs
set actual_model = 'anthropic/' || actual_model
where actual_model is not null
  and actual_model != 'mixed'
  and instr(actual_model, '/') = 0
  and adapter_snapshot = 'claude_code';
