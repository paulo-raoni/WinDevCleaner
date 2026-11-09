# Keeping agents away from the commands that change the disk

An AI agent connected to WinDevCleaner can scan, explain and draft plans freely, but it can never
apply, restore or purge: through MCP, `request_apply` asks the person in the app, and on the
command line `wdc apply`, `wdc restore` and `wdc purge` need proof that a person is present
(Windows Hello, or a one-time code shown only in a desktop dialog). See section 7 of
[the rebuild plan](rebuild-plan.md).

To stop an agent from even trying those commands, add this to Claude Code's settings
(`.claude/settings.json` in a project, or `~/.claude/settings.json` for every project):

```json
{
  "permissions": {
    "deny": [
      "Bash(wdc apply:*)",
      "Bash(wdc restore:*)",
      "Bash(wdc purge:*)",
      "Bash(wdc.exe apply:*)",
      "Bash(wdc.exe restore:*)",
      "Bash(wdc.exe purge:*)"
    ]
  }
}
```

This is a convenience only. Such rules match the text of a command, so a full path to
`wdc.exe`, `cmd /c` or a script gets around them. The barrier is the proof of presence, which the
agent cannot produce; Windows Hello is the barrier against an agent that can also read the screen.
