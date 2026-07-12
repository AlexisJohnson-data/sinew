# Remote mobile Sinew — questions plan mode, plan approval, stop, projets, relay auto-hébergé

## Contexte

Impossible de répondre aux questions du mode plan depuis le téléphone → le travail à
distance est bloqué. Diagnostic (confirmé dans le code) : trois causes cumulées.

1. **Pas de resync après reconnexion WS** — `initialSync` ne tourne qu'une fois
   (`synced` jamais réinitialisé, `app.js:852`). iOS/Android tuent le WebSocket de la
   PWA en arrière-plan ; tous les événements diffusés pendant ce temps (dont le
   `tool_started` de la question) sont perdus (relay stateless, broadcast
   fire-and-forget).
2. **Replay gaté sur un état périmé/racé** — `openConversationById` (`app.js:934`) ne
   rejoue les événements du turn actif que si `activeTurnsRef.current` le dit actif ;
   après reconnexion la liste est vide, et sur le chemin deep-link le ref n'est pas
   encore synchronisé (useEffect pas exécuté) → replay sauté, question invisible
   (l'historique marque tous les tool calls `done`, `app.js:326`).
3. **Aucune push quand une question est posée** — `forward_agent_event` ne notifie
   que sur `TurnFinished` ; en plan mode le turn bloque dans le question tool, donc
   jamais de notification.

Contrainte structurante : la PWA mobile est servie par le relay de Paseru
(`remote.sinew-ide.com`) — toute modif UI mobile impose d'**auto-héberger le relay**
(`remote/` : server.mjs + PWA, autonome ; `relay_url` déjà configurable côté Rust).
Décisions utilisateur : auto-hébergement Railway + 4 features (fix questions, plan
approval, stop turn, push question, switch/ouverture de projet).

## B. Côté PC (Rust — part dans le MSI)

### B1. Push "Question awaiting answer" — `src-tauri/src/remote.rs`
Dans le spawn de `forward_agent_event` (l.1316), après `broadcast_payload` : si
`AgentEvent::ToolStarted { name == "question" }` (constante
`sinew_app::tool_names::QUESTION`), appeler `notify_question_asked(&conversation_id)`
— clone de `notify_turn_finished` (l.1049), body "Question awaiting answer", même
deep-link `conversationId` (déjà géré par `sw.js`). Top-level seulement (les
questions de subagents arrivent en `SubAgentEvent`, exclues volontairement).

### B2. Commande CancelTurn — `remote.rs`
Variante `RemotePhoneCommand::CancelTurn { conversation_id }` (enum l.1489, tag serde
`cancel_turn`) + bras dans `execute_phone_command` (l.672) → réutiliser
`turns::cancel_turn` (turns.rs:743) ; retour `{ "cancelled": ok }`. Les événements
`Interrupted`/`TurnFinished` sortent par le flux normal.

### B3. Plan approval — rien à faire côté PC
`SendMessage.plan_control` existe déjà (remote.rs:1519, valeurs camelCase
`"implementPlan"`/`"updatePlan"`/`"stopQuestions"`, cf. `PlanControlInput`
models.rs:606) et est passé à `turns::send_message`. Vérifier en E seulement.

### B4. Recents persistés + OpenWorkspace — `workspace.rs` + `remote.rs`
- `workspace.rs` : clé `recent_workspaces_v1` dans SQLite `app_settings` via
  `store.load_json_setting`/`save_json_setting` (store.rs:1161/1180, même mécanisme
  que RemoteSettings). Struct `RecentWorkspaceRecord { path, name, last_opened_ms }`,
  dédup par path, tri desc, cap 20. **Hook principal : dans `open_workspace`**
  (l.4-43) après `bootstrap_workspace`. + commande `seed_recent_workspaces` (merge)
  appelée une fois au démarrage depuis `App.tsx` avec `loadRecents()` (recents.ts est
  en localStorage, illisible côté Rust).
- `remote.rs` : `ListRecentWorkspaces` + ajout additif `"recentWorkspaces"` dans la
  réponse `Bootstrap` (l.687). `OpenWorkspace { workspace_path }` : **sécurité —
  n'accepter que si le path normalisé (`normalize_workspace_root`) matche exactement
  une entrée des recents** ; puis `remote_bootstrap` (headless, l.1584) +
  `set_window_workspace("remote", id)` (label synthétique) ; réponse au même format
  que `Bootstrap`.
- **Refactor requis** : `execute_phone_command` résout le workspace AVANT le match
  (l.650-670) et échoue si aucun n'est ouvert — `OpenWorkspace`/`ListRecentWorkspaces`
  (et Ping/SubscribePush) doivent matcher avant cette résolution, sinon impossible
  d'ouvrir un projet quand aucune fenêtre n'est ouverte (le cas d'usage principal).

### B5. Feature flags (compat protocole)
`"features": ["cancel_turn", "open_workspace", "question_push", "plan_control"]`
dans la réponse `Bootstrap`. Sans ça, une commande inconnue envoyée à un vieux PC
fait échouer la déserialisation d'enveloppe → réponse `request_id: "unknown"` → la
promesse PWA pend 60 s. La PWA cache les boutons si le flag est absent.

### B6. ActiveTurns à la connexion du phone
Bras `PhoneConnected` (remote.rs:407) : envoyer à ce device un
`ActiveTurnsChanged` ciblé (via `active_turn_summaries_from_map`, turns.rs:962).
Améliore aussi la vieille PWA. Un seul envoi ciblé, pas de coût par événement.

## C. Côté PWA (`remote/public/app.js` — part via notre relay)

### C1. Fix replay/resync (le bug principal)
- `openConversationById` : **toujours** appeler `replay_active_turn_events` après
  `load_conversation` et se fier au flag `replay.active` de la réponse
  (turns.rs:826/847) au lieu de `activeTurnsRef` — événements live remplacés si
  actif, supprimés sinon. Élimine staleness + race deep-link.
- `resync()` throttlé (~2 s) : bootstrap (workspace courant) + si conversation
  ouverte, load + replay complet (idempotent, remplace liveEvents). Déclencheurs :
  `pcReachable` false→true, et `visibilitychange` → visible.

### C2. Bandeau plan approval
Si `conv.planWorkflow?.status === "planReady"` && pas de streaming : bandeau
au-dessus du composer, boutons **Implement plan** / **Update plan**, calqués sur
`handlePlanImplement`/`handlePlanKeepUpdating` (ChatPane.tsx:2655-2683) :
- Implement : `send_message` texte canned + attachement du plan
  (`artifact.absolutePath`, passthrough path supporté par `RemoteAttachmentInput`
  remote.rs:1614), `mode: "act"`, modèle act de `conv.modeModelSettings`,
  `plan_control: "implementPlan"`, `message_visibility: "systemReminder"` ; puis
  bascule mode act.
- Update : message canned desktop, `mode: "plan"`, `plan_control: "updatePlan"`.

### C3. Bouton Stop
Pendant `isStreaming` (header, à côté de Compact, l.1310) →
`cmd({ type: "cancel_turn", conversation_id })`. Gaté sur `features`.

### C4. Recents dans le menu workspace
Section "Recent" dans `ws-menu` (l.1240) pour les entrées absentes de `workspaces`,
tap → `open_workspace`, appliquer la réponse comme `switchWorkspace` (l.914). Gaté
sur `features`.

### C5. MODEL_CATALOG (l.19-36)
Miroir de models.ts : + `anthropic:claude-sonnet-5` (Sonnet 5, thinking
off/low/medium/high/max) et `openai:gpt-5.6-sol|terra|luna` (off→max, xhigh inclus).

### C6. Bump `CACHE_NAME` dans `sw.js` (v3).

## D. Déploiement Railway + bascule

1. `npx web-push generate-vapid-keys`.
2. Railway : deploy du repo, **Root Directory = `remote`**, variables
   `VAPID_PUBLIC_KEY`/`VAPID_PRIVATE_KEY`/`VAPID_SUBJECT`, ne pas fixer PORT, single
   replica (état en mémoire). Desktop → `wss://<host>/ws`.
3. Bascule desktop — piège : `RemoteSettings::normalized()` (remote.rs:1125) ne
   remplace qu'une URL vide, donc changer `DEFAULT_RELAY_WS_URL` (l.16) ne migre pas
   l'existant. Faire les deux : nouveau défaut + migration dans `normalized()` (si
   `relay_url == ancien défaut` → réécrire), **et** ajouter un champ "Relay URL" dans
   `RemotePanel.tsx` branché sur `ipc.remoteSetEnabled(enabled, relayUrl)`
   (ipc.ts:758 ; `normalize_relay_url` accepte une URL https nue).
4. Re-pairer le téléphone (nouvelle origine = localStorage vide), réactiver la push
   (nouvelle clé VAPID), révoquer les vieux devices dans RemotePanel.

## E. Vérification (dev d'abord — pas de MSI sans demande explicite)

Boucle dev sans téléphone : `node server.mjs` dans `remote/` (localhost:8787),
`npm run tauri dev`, pointer le desktop sur `http://localhost:8787` (champ Relay URL
de D3), "téléphone" = onglet Chrome sur `http://localhost:8787` (localhost = secure
context, crypto.subtle OK ; une IP LAN en http ne marchera PAS).

1. **Questions** : plan mode → panneau apparaît en live ; rejouer avec DevTools
   Offline pendant la question → online → resync restaure le panneau ; réponse OK ;
   deep-link `?conversation=` pendant un turn actif → replay OK ; "Answer & stop"
   passe toujours `stop_questions`.
2. **Push question** : notification au moment de la question → clic → conversation
   avec panneau visible.
3. **Plan approval** : planReady → bandeau ; Implement → turn act avec plan attaché ;
   Update → retour aux questions.
4. **Stop** : turn long → Stop → Interrupted/TurnFinished des deux côtés.
5. **Recents/open** : ouvrir un projet non ouvert sur le PC depuis la PWA ; path hors
   recents envoyé à la main → rejeté.
6. **Compat vieille PWA** contre nouveau PC (changements additifs uniquement).
7. Déploiement Railway + test réel téléphone (pair, question+push, stop, open).
   MSI ensuite, sur demande.

## Risques

- Restructuration `execute_phone_command` touche tous les chemins de commande →
  re-tester toutes les commandes existantes avec la vieille PWA.
- iOS : Web Push exige la PWA installée (écran d'accueil, iOS 16.4+) ; le clic
  notification recharge l'app à froid — c'est le fix deep-link/replay qui rend ce
  chemin fonctionnel. Testable seulement après le deploy Railway.
- `set_window_workspace("remote", …)` écrase `current_workspace` ; un focus de
  fenêtre desktop le rebascule — sans gravité (la PWA passe toujours un workspace
  explicite), à tester avec deux fenêtres ouvertes.
- Redémarrages Railway : les deux côtés se reconnectent (PC 2 s, PWA 1,5 s) et le
  resync C1 couvre le trou côté téléphone.
