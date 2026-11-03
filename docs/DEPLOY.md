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
reaches the login page, though registration still needs an invite. Use it for testing, then deploy as below.

## What you need

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

Each person needs their own invite. Create one per family member:

```sh
docker compose -f deploy/compose.yml --env-file deploy/.env exec app mm-server invite
```

Open `https://your-domain` on your phone, choose **Create account**, and paste the invite.
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
