# Sinew Remote relay — setup guide

> **Purpose:** This folder is a small relay plus mobile PWA for Sinew Remote.
> The Sinew desktop app on your PC remains the agent host: it runs models,
> tools, terminal commands, browser automation, and accesses local projects.
> The relay only routes encrypted traffic between your PC and paired phone.
>
> **Important:** This is remote control, not a cloud agent. Your PC must be
> powered on and online for the phone to work.

## Give this prompt to an LLM

Copy this prompt when you want help without exposing secrets:

> Read `remote/README.md` in my Sinew fork and guide me one click at a time to
> deploy the `remote/` relay on Railway, configure Sinew Remote, and pair my
> phone. Ask for screenshots when UI labels differ. Never ask me to paste, send,
> commit, or publish VAPID private keys, pairing codes, device tokens, API keys,
> `.env` files, or personal credentials.

## What the relay can and cannot see

- The relay serves the mobile web app and routes WebSocket frames.
- Phone ↔ PC commands and events are encrypted end-to-end.
- The relay does **not** decrypt chat messages or inspect project files.
- A paired phone can instruct the Sinew agent on the PC. Pair only devices you
  control, and revoke a lost phone immediately from **Sinew → Remote**.

## Quick architecture

```text
Phone PWA  ← encrypted WebSocket →  relay  ← encrypted WebSocket →  Sinew desktop on PC
                                                        ↓
                                             optional Web Push delivery
```

The relay keeps connected-PC and pairing state in memory. Use **one replica**.
Multiple replicas may route a phone and its PC to different instances.

## Run locally (development)

Requirements: Node.js 18 or newer.

```powershell
cd remote
npm install
npm start
```

The local relay runs at `http://localhost:8787`.

For a development test:

1. Start the relay with the command above.
2. Start Sinew desktop with `npm run tauri dev` from the repository root.
3. In **Sinew → Remote**, set Relay URL to `http://localhost:8787` and enable Remote.
4. Open `http://localhost:8787` in Chrome as a test phone, open pairing on the PC, then pair.

> `localhost` is a secure context for browser crypto. A plain `http://` LAN IP
> is not suitable for the installed-PWA/push flow.

## Deploy on Railway

### Before you start

- Push the repository branch you want Railway to deploy to GitHub.
- The relay source is the `remote/` directory, not the repository root.
- Keep secrets in Railway Variables only; do not commit them.

### Step 1 — Create the service

1. In Railway, create a project from your GitHub repository.
2. Select the desired branch.
3. Open the service **Settings**.
4. Under **Source**, set **Root Directory** to exactly:

   ```text
   remote
   ```

5. Leave Railway's detected build/start commands alone. `remote/package.json`
   already provides `npm start` (`node server.mjs`).
6. In **Scale**, keep **1 replica**.
7. Do **not** create a database, volume, Docker image, or custom `PORT` variable.
8. Keep Serverless disabled: the connection routing state is in memory and the
   desktop needs a stable WebSocket endpoint.

Railway supplies `PORT` itself. Never add or override it.

### Step 2 — Generate a public domain

1. In **Settings → Networking**, click **Generate Domain**.
2. Copy the HTTPS domain, for example:

   ```text
   https://your-relay.up.railway.app
   ```

3. The desktop Relay URL uses WebSocket form with `/ws` appended:

   ```text
   wss://your-relay.up.railway.app/ws
   ```

### Step 3 — Configure optional Push notifications

Push is optional. Pairing, chat, streaming, questions, Stop, and project
switching work without it. Push adds alerts when a question needs an answer or
a turn completes.

Generate a new VAPID key pair locally:

```powershell
npx web-push generate-vapid-keys
```

In **Railway → service → Variables**, add:

| Name | Value |
| --- | --- |
| `VAPID_PUBLIC_KEY` | Generated public key |
| `VAPID_PRIVATE_KEY` | Generated private key — keep secret |
| `VAPID_SUBJECT` | A contact URL, usually `mailto:you@example.com` |

`VAPID_SUBJECT` must include `mailto:`. For example:

```text
mailto:you@example.com
```

Do not put VAPID keys in GitHub, screenshots, chat messages, source code, or
`.env` files that are tracked by git. Railway redeploys after a variable change.

### Step 4 — Confirm health

After the deployment is green/online, visit:

```text
https://your-relay.up.railway.app/healthz
```

Expected response:

```json
{ "ok": true }
```

Visiting the domain root should show the Sinew Remote mobile page.

## Configure the Sinew desktop app

1. Launch the custom Sinew build.
2. Open **Remote**.
3. Paste the relay URL:

   ```text
   wss://your-relay.up.railway.app/ws
   ```

4. Click **Save**.
5. Enable the Remote switch.
6. Wait for **Relay: Connected** and **Reachable: Yes**.
7. Click **Open pairing**.

If you change to a different relay domain/origin later, pair the phone again.

## Pair and install the phone PWA

1. On the phone, open the public HTTPS Railway domain:

   ```text
   https://your-relay.up.railway.app
   ```

2. On the PC, leave the pairing panel open.
3. On the phone, scan the QR code or enter the six-digit pairing code.
4. Confirm the pair request if prompted.
5. Install the page as a PWA:
   - **iPhone:** Safari → Share → Add to Home Screen.
   - **Android:** browser menu → Install app / Add to Home screen.
6. Open the installed app and verify it says **PC connected**.
7. Press **Push** and allow notifications if VAPID was configured.

On iPhone, Web Push requires the PWA to be installed to the home screen.

## What to test

1. Send a normal message from the phone.
2. Start a long task, then use **Stop** in the mobile chat header.
3. In Plan mode, cause Sinew to ask a question. Confirm the phone shows the
   choices and can send an answer.
4. Put the phone/browser in the background during a question, return, and
   reopen the conversation. The question should replay.
5. Test **Implement plan** and **Update plan** after a plan is ready.
6. Open the workspace menu on the phone and switch to a recent project. A
   project that was closed on the PC can be opened headlessly if it is stored
   in the PC's recent-project list.
7. Send a question and verify a push alert arrives, if Push is enabled.

## Troubleshooting

### Railway build fails immediately

Most commonly Railway is building the repository root. Set:

```text
Settings → Source → Root Directory → remote
```

Then redeploy.

### `Vapid subject is not a valid URL`

Set `VAPID_SUBJECT` with a scheme:

```text
mailto:you@example.com
```

Not just `you@example.com`.

### Phone says PC unreachable

Check all of these:

- Desktop Remote is enabled.
- Desktop Relay URL is the `wss://.../ws` URL for the same relay domain.
- Railway service is online.
- The phone opened the matching `https://...` relay domain.
- The PC is awake, has internet, and Sinew is running.

### A question does not appear on the phone

Refresh/reopen the mobile PWA, return to the conversation list, then open the
conversation again. Active turns replay their events after reconnecting.

### `this workspace is no longer open on the PC`

Refresh the PWA after deploying the current version. The current mobile client
falls back to a secure headless open for stored recent projects. Arbitrary paths
are intentionally rejected; open the project once on the PC first so it enters
recent projects.

### Push does not work

- Install the PWA on the phone home screen first, especially on iPhone.
- Confirm all three VAPID variables are set on Railway.
- Confirm `VAPID_SUBJECT` starts with `mailto:`.
- Use the **Push** button in the mobile app and grant permission.
- Re-pair/re-enable Push after changing relay domain or VAPID keys.

## Security checklist

- [ ] Use a private repository if the source itself is private.
- [ ] Keep `VAPID_PRIVATE_KEY`, pairing codes, device tokens, model API keys,
      OAuth tokens, and `.env` files out of Git.
- [ ] Keep one Railway replica.
- [ ] Pair only devices you own.
- [ ] Revoke devices immediately if lost, sold, or replaced.
- [ ] Use an HTTPS/WSS relay domain in production.
- [ ] Remember that the paired phone can direct the agent operating on your PC.

## No database or volume needed

The relay is intentionally stateless: no database and no persistent volume are
required. If Railway restarts, the desktop and phone reconnect automatically.
The desktop keeps paired-device records and the phone keeps its pairing session.
