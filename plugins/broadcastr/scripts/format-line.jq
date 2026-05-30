def tier_icon:
  if .tier == "alert" then "🚨" else "📡" end;

# Category → glyph. Group related categories to reduce visual noise.
def category_glyph:
  if   .category == "agent-presence"                                  then "👤"
  elif (.category | test("^(commit|push|pre-commit|branch|stash)$")) then "🌿"
  elif (.category | test("^(session-doc|plan|plan-exec)$"))          then "📝"
  elif .category == "bead"                                            then "🎯"
  else "•" end;

# Prefer data.agent (set by session hooks) over emitter.agent mapping.
def agent_name:
  .data.agent //
  (if .emitter.agent == "claude-code" then "Claude" else "Claude" end);

# Human-readable summary that always leads with the agent name.
def display_summary:
  agent_name as $ag |
  if .category == "agent-presence" then
    $ag + (if (.data.action // "joined") == "left" then " left" else " joined" end)

  elif .category == "session-doc" then
    (if .data.path then (.data.path | split("/") | last)
     else (.summary | ltrimstr("session doc: ")) end) as $fname |
    $ag + " saved: " + $fname

  elif (.category == "plan" or .category == "plan-exec") then
    (if .data.path then (.data.path | split("/") | last)
     else (.summary | ltrimstr("plan edit: ") | ltrimstr("plan-exec: ")) end) as $fname |
    $ag + " edited: " + $fname

  elif .category == "commit" then
    if (.data.subtype // "") == "merge" then
      $ag + " merged · " + (.branch // "?")
    else
      $ag + " made commit " + (.summary | ltrimstr("commit "))
    end

  elif .category == "push" then
    if (.data.subtype // "") == "attempt" then
      $ag + " pushing · " + (.branch // "?")
    elif (.tier == "alert") or (.summary | test("FAIL"; "i")) then
      $ag + "’s push FAILED · " + (.branch // "?")
    else
      $ag + " pushed · " + (.branch // "?")
    end

  elif .category == "pre-commit" then
    if (.tier == "alert") or (.summary | test("FAIL"; "i")) then
      $ag + " pre-commit FAILED · " + (.branch // "?")
    elif (.summary | test("pass"; "i")) then
      $ag + " pre-commit ✓ · " + (.branch // "?")
    else
      $ag + " pre-commit starting · " + (.branch // "?")
    end

  elif .category == "branch" then
    $ag + " switched to · " + (.data.branch // (.summary | ltrimstr("checkout: ")))

  elif .category == "bead" then
    ((.data.cmd // .summary) | ltrimstr("bd: bd ") | ltrimstr("bd ") | split(" ")) as $parts |
    ($parts[0] // "?" |
      if   . == "close"  then "closed"
      elif . == "create" then "created"
      elif . == "update" then "updated"
      elif . == "reopen" then "reopened"
      else . end) as $verb |
    ($parts | map(select(test("^beads-"))) | .[0] // "") as $issue |
    $ag + " " + $verb + (if $issue != "" then " " + $issue else "" end)

  elif .category == "stash" then
    $ag + " stashed · " + (.branch // "?")

  else .summary
  end;

select($sid == "" or .emitter.session_id == null or .emitter.session_id != $sid)
| select(.ts > $startup)
| select(.category as $c | $mute | index($c) | not)
| ((.repo // "") | split("/") | last) as $proj
| tier_icon + " " + category_glyph + "[" + $proj + "] " + display_summary
