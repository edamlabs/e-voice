# Claude Code with a voice

`evoiced` is the local daemon that owns the microphone and the speaker; Claude Code reaches it three ways. Start
the STT and TTS services, then the daemon:

```
evoiced serve
```

It listens on `127.0.0.1:5700`.

## Tools — MCP

```
claude mcp add --transport http evoice http://127.0.0.1:5700/mcp
```

Claude gets `speak`, `stop`, `listen` and `list_voices`. To give this registration its own voice, name it and map
the name in `evoice.toml`:

```
claude mcp add --transport http evoice http://127.0.0.1:5700/mcp --header "X-Evoice-Client: claude"
```

```toml
[daemon.voices]
claude = "<voice>"
```

The name is a preference, never a permission. `/mcp` answers only to a loopback `Host`, even when `[daemon] host`
is widened.

## Spoken replies — hooks

Add to a Claude Code `settings.json`:

```json
{
  "hooks": {
    "Stop": [
      { "hooks": [ { "type": "http", "url": "http://127.0.0.1:5700/hooks/stop", "timeout": 5 } ] }
    ],
    "Notification": [
      { "matcher": "permission_prompt",
        "hooks": [ { "type": "http", "url": "http://127.0.0.1:5700/hooks/notification", "timeout": 5 } ] },
      { "matcher": "idle_prompt",
        "hooks": [ { "type": "http", "url": "http://127.0.0.1:5700/hooks/notification", "timeout": 5 } ] }
    ]
  }
}
```

- When Claude stops, the daemon speaks the first two sentences of its reply: prose only — code, tables, headings
  and URLs are left out, a path is read as its file name. The language comes from the words, else `[daemon] lang`.
- When Claude waits for a permission or has been idle, it says so.
- Each hook answers 200 with an empty body at once — or, for an input over 1 MiB, the connection is closed once
  the daemon stops reading at the cap; either way nothing is said and Claude Code goes on. Speech never slows
  Claude Code. If the daemon is not running, Claude Code shows its own non-blocking hook error.
- Every stop is spoken. If another Stop hook sends Claude back to work, the closing line of each attempt is heard.
- For a voice of its own, add `"headers": {"X-Evoice-Client": "claude"}` to each hook.

## Dictation

`ecli listen` waits for you to start speaking and prints what you said. Bind it to a desktop shortcut that types
the text where the cursor is. It is press-to-start, not hold-to-talk: it ends by itself when you finish a sentence,
and the text is typed without Enter, so you review it before sending.

- X11 (GNOME: Settings → Keyboard → Custom Shortcuts):
  `sh -c 'ecli listen | tr -d "\n" | xdotool type --clearmodifiers --file -'`. Needs `xdotool`.
- Wayland, wlroots compositors (sway, Hyprland): `sh -c 'ecli listen | tr -d "\n" | wtype -'`. Needs `wtype`.
- Not verified: that `xdotool type --file -` and `wtype -` read from standard input as their documentation says,
  and any GNOME or KDE Wayland session. GNOME's Wayland compositor does not offer the protocol `wtype` uses;
  there, typing would need `ydotool`, which runs as root, or the desktop portal.
- Hold-to-talk is not built: it needs a global key reader and a listen that ends on release.
