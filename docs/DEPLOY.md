# Deploying to a VPS

This puts the app on your own server with HTTPS, so you can install it on your phone and scan receipts.
It takes about 15 minutes, most of it waiting for the first build.

> **This is a test deployment.** Automatic encrypted backups are not set up yet.
> Keep real data in it only if you accept that losing the server means losing the server copy.
> Your phone keeps its own encrypted copy, which syncs back when the server returns.

## Trying it on a phone without a VPS

A phone cannot use `http://<computer IP>:8080`: on a plain HTTP page the browser turns off passkeys, the camera
and safe storage, and the app shows a warning instead. Passkeys also need a domain name, never an IP address.
A temporary HTTPS address from Cloudflare solves both, for free and without an account:

```sh
brew install cloudflared                       # once, on the computer that runs the app
docker compose up --build -d
cloudflared tunnel --url http://localhost:8080
```

It prints an address like `https://quiet-river-1234.trycloudflare.com`. Restart the app with that name,
in a second terminal, then open the address on the phone:

```sh
RP_ID=quiet-river-1234.trycloudflare.com ORIGIN=https://quiet-river-1234.trycloudflare.com docker compose up -d
```

Limits: the address changes each time `cloudflared` starts, and a passkey made for one address does not work
on the next. Cloudflare carries the traffic, which it can see only as ciphertext, and anyone with the address
can create an account there. Use it for testing, then deploy as below.

## A fixed test address on your own computer (Cloudflare Tunnel)

The app runs in Docker on your computer and gets a permanent address such as `https://mm.example.com`, with a real
certificate, so passkeys work on phones. Data stays in the Docker volume across rebuilds. The domain must use
Cloudflare's name servers (add it at dash.cloudflare.com first, if it is not there yet).

1. In the Cloudflare dashboard open **Zero Trust → Networks → Tunnels → Create a tunnel**, choose **Cloudflared**,
   and name it, for example `mm-mac`. Copy the token from the install command shown (the long string after
   `--token`). Skip installing anything: the tunnel runs in Docker.
2. On the tunnel's **Public Hostname** tab, add a hostname: subdomain `mm`, your domain, service type `HTTP`,
   URL `app:8080`. Cloudflare creates the `mm` CNAME record itself.
3. Next to `compose.yml`:

   ```sh
   cp .env.example .env     # then fill in TUNNEL_TOKEN, RP_ID and ORIGIN with your hostname
   docker compose up --build -d
   ```

4. Open the address on the phone and choose **Sign up**.

To update after `git pull`, run `docker compose up --build -d` again: accounts, passkeys and data stay.
Never run `docker compose down -v`, which deletes the database volume. Keep `RP_ID` the same: passkeys belong to
that name. A copy of the test database: `docker compose exec -T db pg_dump -U mm mm > mm-backup.sql`.

The computer must stay on and awake for the address to work. Cloudflare ends TLS on its side: it sees records only
as ciphertext, but it serves the app's code to the phone and could change it, as noted in the README.
Use a VPS, below, for real family data.


- A VPS in the EU with at least 2 GB of RAM and 10 GB of disk, running Ubuntu 24.04 or Debian 12.
  1 GB of RAM is not enough for the first build.
- A domain or subdomain, such as `mm.example.com`. Passkeys and the camera need HTTPS, and HTTPS needs a domain.
- SSH access to the server.

## 1. Point the domain at the server

At your domain registrar, create an **A record** for the name you chose, with your server's IP address.
Check that it works before continuing. This should print the server's IP:

```sh
dig +short mm.example.com
```

## 2. Install Docker on the server

```sh
ssh root@your-server-ip
curl -fsSL https://get.docker.com | sh
```

Open ports 80 and 443 if your provider has a firewall. Port 80 is needed for the certificate, then it redirects to HTTPS.

## 3. Get the app and configure it

```sh
git clone https://github.com/dthemn/mm.git
cd mm
cp deploy/.env.example deploy/.env
sed -i "s/^POSTGRES_PASSWORD=.*/POSTGRES_PASSWORD=$(openssl rand -hex 24)/" deploy/.env
nano deploy/.env   # set DOMAIN to your domain
```

## 4. Start it

```sh
docker compose -f deploy/compose.yml --env-file deploy/.env up -d --build
```

The first build takes 10 to 20 minutes. Caddy then gets a certificate on its own.
Check the logs if something seems stuck:

```sh
docker compose -f deploy/compose.yml --env-file deploy/.env logs -f
```

## 5. Create an account

Open `https://your-domain` on your phone and choose **Sign up**. Each person creates their own account.
Registration is open: anyone who can reach your address can create an account and store encrypted data there.
Write down the 12 words on paper: without them and the passkey, the data cannot be recovered.

To install the app on the home screen:

- **iPhone, Safari:** Share, then **Add to Home Screen**.
- **Android, Chrome:** the menu, then **Install app** or **Add to Home screen**.

The first receipt scan downloads the recognizer, about 26 MB, once. After that, scanning works offline.

## Updating

```sh
cd mm
git pull
docker compose -f deploy/compose.yml --env-file deploy/.env up -d --build
```

## What the server stores

Only encrypted records, their sizes, revision numbers and update times rounded to the hour.
Receipt photos never reach it. Server logs hold the request method, path and status, without bodies or cookies.

## Known limits

- No automatic backups yet. Restoring an old database copy also needs a sync change that is not done yet.
- Recognition was tuned on a few receipts. Expect to correct some fields, especially on crumpled or faded paper.
- Passkey encryption (PRF) was tested only with a virtual passkey in desktop Chrome.
  Settings shows whether it works on your phone: "Passkey encryption: on" or "off".
