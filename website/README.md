# rusttalk.com

Static project website (English/German, no build step, no cookies, no tracking).

| File | Purpose |
|---|---|
| `index.html` | Home page: features, screenshots, downloads, server setup |
| `install.html` | Step-by-step install guides for Windows, Linux and the Steam Deck (keep in step with the README's client sections) |
| `roadmap.html` | Planned, maybe-later and shipped features (update it when a release ships something new) |
| `impressum.html`, `datenschutz.html` | Legal notice and privacy policy (required in Germany) |
| `legal.css`, `lang.js` | Shared style and DE/EN switch for the legal pages |
| `img/` | Screenshots and logo PNGs |
| `Caddyfile` | Web server config (HTTPS via Let's Encrypt, 80 → 443) |
| `deploy.sh` | Uploads the site to the server |

Download buttons default to the GitHub releases page. The inline release loader resolves exact
assets from stable client/server or legacy combined releases, ordered by numeric
version. Build metadata does not affect order; component tags take precedence over
same-version legacy tags. Ambiguous component versions refuse discovery before
asset filtering. Index-server releases do not qualify. History menus show only
releases containing files for their component and platform; checksums remain visible.

The browser reads the existing GitHub API (disclosed in `datenschutz.html`), using
at most 10 requests of 100 releases, with a streamed 2 MiB response limit per page
and a 15-second overall deadline. URLs are constructed locally; pagination links
are never followed. Incomplete pagination, malformed responses, errors, ambiguity
or missing assets leave safe releases-page links. Downloads accept only exact
HTTPS asset URLs in `rusttalkcrew/RustTalk`. No API strings become HTML.

The server setup example deliberately asks users to choose a stable server or
combined release and download its Linux archive before unpacking. It does not
assume the repository-wide latest release contains a server.

## Offline checks and integration

Run `make -C website test` (Node and Python standard libraries only).
`tests/integrate.py INPUT OUTPUT` applies the focused loader/link edit to either
the older bound HTML or the newer main HTML. It preserves the OS chooser, layout,
architecture buttons and history containers; it replaces only release discovery,
static download links and the server download instruction. When cherry-picking
across the newer main website, resolve `index.html` with this transform against
that version, not by replacing the whole file with the older checkout's HTML.
The integration test reads main through `git show` into a temporary fixture and
checks preservation. It does not edit another checkout.

## Preview

Open `index.html` in a browser, or serve the folder with any static file server.

## Deploy

```bash
RUSTTALK_WEB_HOST=user@your-server RUSTTALK_WEB_PORT=22 bash website/deploy.sh
```

First-time server setup (Debian/Ubuntu):

```bash
apt install caddy
cp website/Caddyfile /etc/caddy/Caddyfile && systemctl reload caddy
```

DNS: `rusttalk.com` may be proxied by Cloudflare. A RustTalk voice server on the same host needs its own **DNS only** record, because the Cloudflare proxy does not forward port 7331 or UDP.

## Screenshots

Take them with neutral names and no private desktop content (e.g. a local dev server with test clients). Keep them around 1100 px wide.
