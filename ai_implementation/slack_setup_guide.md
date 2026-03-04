# Slack Agent Setup Guide

Follow these steps to create a Slack app, get the required tokens, and configure the orchestrator.

---

## Step 1: Create a Slack App

1. Go to **https://api.slack.com/apps**
2. Click **"Create New App"**
3. Choose **"From scratch"**
4. Name it something like `Orchestrator Triage Bot`
5. Select your workspace
6. Click **"Create App"**

## Step 2: Enable Socket Mode

Socket Mode lets the bot receive events over a WebSocket instead of requiring a public HTTP endpoint.

1. In the app settings sidebar, go to **"Socket Mode"**
2. Toggle **"Enable Socket Mode"** → ON
3. You'll be prompted to create an **App-Level Token**:
   - Token name: `orchestrator-socket`
   - Scope: `connections:write` (should be pre-selected)
   - Click **"Generate"**
4. **Copy the `xapp-...` token** — this is your `SLACK_APP_TOKEN`
5. Save it somewhere safe (you'll need it in Step 6)

## Step 3: Add Bot Token Scopes

1. Go to **"OAuth & Permissions"** in the sidebar
2. Scroll to **"Scopes" → "Bot Token Scopes"**
3. Add these scopes:

| Scope | Purpose |
|---|---|
| `channels:history` | Read messages in public channels |
| `channels:read` | List public channels |
| `chat:write` | Post to your notification channel |
| `users:read` | Resolve usernames for display |
| `groups:history` | Read private channels (if watching private channels) |
| `groups:read` | List private channels |

## Step 3b: Add User Token Scopes (for DM/private channel access)

To let the bot see **your** DMs, group DMs, and private channels as if it were you, add User Token Scopes:

1. Still in **"OAuth & Permissions"**
2. Scroll to **"Scopes" → "User Token Scopes"**
3. Add these scopes:

| Scope | Purpose |
|---|---|
| `im:history` | Read your DMs |
| `im:read` | List your DM conversations |
| `mpim:history` | Read your group DMs |
| `mpim:read` | List your group DM conversations |
| `channels:history` | Read channels you're in |
| `channels:read` | List channels you're in |
| `groups:history` | Read private channels you're in |
| `groups:read` | List private channels you're in |

> **Why User Token?** Bot tokens only see conversations the bot is a member of.
> A User Token lets the app receive events for all conversations *you* are part
> of — including DMs people send you, group chats you're in, and private channels
> you belong to. The bot effectively sees Slack through your eyes.

## Step 4: Subscribe to Events

1. Go to **"Event Subscriptions"** in the sidebar
2. Toggle **"Enable Events"** → ON
3. Under **"Subscribe to bot events"**, add:

| Event | Purpose |
|---|---|
| `message.channels` | New messages in public channels |
| `message.groups` | New messages in private channels |
| `app_mention` | When someone @mentions the bot |

4. Under **"Subscribe to events on behalf of users"**, add:

| Event | Purpose |
|---|---|
| `message.im` | New DMs to/from you |
| `message.mpim` | New group DMs you're in |
| `message.channels` | Messages in channels you're in |
| `message.groups` | Messages in private channels you're in |

> These user event subscriptions are what let the bot see your DMs.
> They fire for all conversations the installing user (you) is part of.

5. Click **"Save Changes"****

## Step 5: Install the App to Your Workspace

1. Go to **"Install App"** in the sidebar
2. Click **"Install to Workspace"** (or **"Reinstall to Workspace"** if updating scopes)
3. Review the permissions and click **"Allow"**
4. You'll see **two tokens** on the Install App page:
   - **Bot User OAuth Token** (`xoxb-...`) → this is your `SLACK_BOT_TOKEN`
   - **User OAuth Token** (`xoxp-...`) → this is your `SLACK_USER_TOKEN`
5. **Copy both tokens**

## Step 6: Get Your IDs

### Your Slack User ID

1. In Slack, click on your own profile picture
2. Click **"Profile"**
3. Click the **"⋮" (More)** button
4. Click **"Copy member ID"**
5. This is your `alert_user_id` (format: `U0XXXXXXX`)

### Bot User ID

1. Go back to **https://api.slack.com/apps** → your app
2. Go to **"Basic Information"**
3. Scroll to **"App Credentials"**
4. Note the **App ID** (but you actually need the Bot User ID)
5. Easiest way: in Slack, find your bot in the sidebar → click its name → **"Copy member ID"**
6. This is your `bot_user_id` (format: `U0XXXXXXX`)

### Channel IDs

For each channel you want to watch + your notification channel:

1. In Slack, right-click the channel name
2. Click **"View channel details"** (or **"Open channel details"**)
3. Scroll to the bottom — the Channel ID is shown there (format: `C0XXXXXXX`)
4. **Important**: Make sure the bot is a member of each channel you want to watch AND the notification channel. Invite it with `/invite @Orchestrator Triage Bot`

### Create Your Private Notification Channel

1. In Slack, click **"+"** next to "Channels"
2. Choose **"Create a channel"**
3. Name it something like `#josh-triage-alerts`
4. Toggle **"Make private"** → ON
5. Create it, then invite the bot: `/invite @Orchestrator Triage Bot`
6. Copy the Channel ID (see above)
7. **Enable notifications for this channel** on mobile:
   - Long press (or right-click) the channel → **"Notifications"** → **"Every new message"**
   - This ensures posts here ping your phone

## Step 7: Set Environment Variables

Add to your shell profile (`~/.zshrc`, `~/.bashrc`, etc.):

```bash
export SLACK_BOT_TOKEN="xoxb-your-bot-token-here"
export SLACK_APP_TOKEN="xapp-your-app-token-here"
export SLACK_USER_TOKEN="xoxp-your-user-token-here"
```

Then reload: `source ~/.zshrc`

## Step 8: Create the Config File

The orchestrator expects the Slack config at `.orchestrator/slack_config.toml`.

A template has been created for you at:
```
.orchestrator/slack_config.toml
```

Edit it and fill in your IDs:
- `bot_user_id` — from Step 6
- `watch_channels` — channel IDs from Step 6
- `notification_channel` — your private notification channel ID from Step 6
- `alert_user_id` — your Slack user ID from Step 6

The tokens are read from env vars by default (`SLACK_BOT_TOKEN`, `SLACK_APP_TOKEN`, `SLACK_USER_TOKEN`), so you don't need to put them in the file.

---

## Verification Checklist

Before running:

- [ ] `echo $SLACK_BOT_TOKEN` shows `xoxb-...`
- [ ] `echo $SLACK_APP_TOKEN` shows `xapp-...`
- [ ] `echo $SLACK_USER_TOKEN` shows `xoxp-...`
- [ ] `.orchestrator/slack_config.toml` exists with your IDs filled in
- [ ] `.orchestrator/slack_config.toml` is in `.gitignore`
- [ ] Bot is invited to all watched channels
- [ ] Bot is invited to the notification channel
- [ ] Notification channel has mobile notifications set to "Every new message"
