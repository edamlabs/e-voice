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

## Conversation

`ecli talk` starts a voice conversation with Claude Code through the daemon. It listens, you speak, it answers
aloud, and it listens again. `ecli talk stop`, or saying «adiós» / «goodbye» on its own, ends it. It also ends by
itself after a minute of silence (`[daemon.talk] idle`).

- **It is half-duplex.** While the agent thinks or speaks, the microphone is closed: you cannot talk over it. To cut
  a long answer, run `ecli stop` (or the key you bound to it): the speech stops, the agent drops that turn, and it
  listens again. `ecli stop` silences everything the daemon is saying, not only the conversation.
- **Permissions are asked aloud.** When the agent wants to do something that needs your yes, you hear the request
  word for word, every symbol named: «¿Ejecuto: ls, guion la, barra vertical, head?». Answer «sí» or «no». An
  unclear answer is asked once more; silence is a no. A command that is too long or cannot be read faithfully is
  refused without asking, and the agent is told to simplify it. Nothing you did not hear whole can be approved.
- **It does not use your Claude Code settings.** The session never loads your user settings, hooks, plugins or
  permission rules: with them a command could run unasked. It has only the tools named in
  `[daemon.talk.claude] tools`; every one but reading is asked. The project at `cwd` keeps its own deny rules and
  hooks; its allow rules cannot skip a spoken yes. An organisation's managed settings stand above all of this.
- **Each conversation starts fresh.** A new `ecli talk` is a new agent session. Within one conversation, if the
  agent's process dies, you hear so, and the next thing you say picks the session up again.
- While a conversation runs, `ecli listen` and the `listen` tool answer busy: the microphone is the conversation's.

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
