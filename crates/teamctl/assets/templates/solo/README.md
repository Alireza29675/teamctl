# {{project_name}}

This `.team/` was scaffolded by `teamctl init --template solo`. It defines
one manager and one dev, both running Claude Code, talking through a
SQLite mailbox.

## Run

```bash
cp .env.example .env        # edit if you add interfaces / secrets later
cd ..
teamctl validate
teamctl up
teamctl ps
```

## Send a message

```bash
teamctl send {{project_id}}:manager "summarise the README"
teamctl mail {{project_id}}:manager
teamctl tail {{project_id}}:manager -f
```

## Stop

```bash
teamctl down            # stop tmux sessions; mailbox preserved
rm -rf state/           # full reset
```

## Customize

- Edit `roles/manager.md` and `roles/dev.md` to change the agents'
  voices and operating loops.
- Edit `projects/main.yaml` to add workers, channels, or another
  manager.
- Edit `team-compose.yaml` to add interfaces (Telegram, etc.) or
  rate-limit hooks.

After any edit, `teamctl reload` picks up the change and restarts only
the agents whose configuration was touched.
