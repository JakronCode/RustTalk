# rusttalk.com

Static project website (English/German, no build step, no cookies, no tracking).

| File | Purpose |
|---|---|
| `index.html` | Home page: features, screenshots, downloads, server setup |
| `roadmap.html` | Planned, maybe-later and shipped features (update it when a release ships something new) |
| `impressum.html`, `datenschutz.html` | Legal notice and privacy policy (required in Germany) |
| `legal.css`, `lang.js` | Shared style and DE/EN switch for the legal pages |
| `img/` | Screenshots and logo PNGs |
| `Caddyfile` | Web server config (HTTPS via Let's Encrypt, 80 → 443) |
| `deploy.sh` | Uploads the site to the server |

Download buttons point to `releases/latest/download/...` on GitHub, so a new release needs no website change. The home page reads the latest version number from the GitHub API in the visitor's browser (disclosed in `datenschutz.html`).

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
