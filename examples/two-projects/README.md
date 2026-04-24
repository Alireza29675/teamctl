# Example: two-projects

Two unrelated teams share one teamctl instance. They're **isolated by default** — nothing in `product` can reach `blog` without an explicit bridge.

```bash
teamctl validate
teamctl up

# Bridge the two managers for 2 hours on a specific topic:
teamctl bridge open \
  --from product:manager \
  --to blog:editor \
  --topic "share launch event photos" \
  --ttl 120

teamctl bridge list         # see open / expired / closed bridges
teamctl bridge log 1        # replay the transcript of bridge #1
teamctl bridge close 1
```

While the bridge is open, only the two named managers can DM across. Every message they exchange is recorded with a `thread_id` of `bridge:<id>` for audit.
