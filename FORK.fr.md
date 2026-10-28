# Sinew — Fork personnel

> **TL;DR** — Ce fork ajoute un mode shell WSL/Ubuntu, un agent de migration
> de projet Windows→WSL en un clic, **une vraie automatisation de navigateur
> intégrée à l'agent (CDP/Chromium)**, **la lecture de PDF (texte + pages
> scannées via la vision du modèle)**, **l'ajout de fichiers au contexte de
> la conversation** (glisser-déposer, clic droit, collage depuis le
> presse-papiers), **la citation d'une réponse dans le contexte**, une UX de
> chat plus conviviale (bulles façon iMessage, copie des blocs de code,
> recherche Ctrl+F), des notifications de workflow, Quick Open (Ctrl+P) et
> quelques ajustements de mise en page qui rendent Sinew utilisable quand on
> vit sur Windows mais qu'on code sous Linux.

Construit par-dessus [Paseru/sinew](https://github.com/Paseru/sinew) — chaque
fonctionnalité ici est additive. Tirez les nouveautés d'upstream quand vous
voulez ; le fork reste sur une seule branche `feat/wsl-shell` pour que les
fusions restent propres.

Si vous voulez juste l'**installateur Windows**, récupérez le `.msi` dans les
[Releases](../../releases) et passez directement à *Configurer un fournisseur
de modèle*.

Si vous voulez compiler / personnaliser davantage, lisez la suite.

---

## Pourquoi ce fork existe

Sinew est génial si vous écrivez du code nativement sur le système
d'exploitation sur lequel Sinew tourne. Mais beaucoup d'entre nous (moi
compris) utilisent Windows pour tout **sauf** le dev, et vivent dans WSL2
(`\\wsl$\Ubuntu\home\moi\projects\…`). L'outil `bash` de l'agent doit
exécuter des commandes là où le projet se trouve réellement — lancer
`npm install` depuis PowerShell sur un projet sous `\\wsl$`, c'est la
recette d'un `node_modules` maudit.

Donc :

1. L'agent sait maintenant lancer `wsl.exe -- bash -lc` pour n'importe
   quelle commande, avec un mode `Auto` conscient de l'espace de travail
   qui choisit PowerShell ou WSL **en fonction du chemin de l'espace de
   travail ouvert**.
2. Un **agent de migration** intégré copie un projet hébergé sur Windows
   vers WSL en toute sécurité (git-clone si l'arbre est propre, copie de
   fichiers avec exclusions s'il est sale), met en évidence les risques
   liés aux variables d'environnement et vous donne une liste de contrôle
   post-migration.

Tout le reste dans cette liste, ce sont des améliorations de confort issues
d'une utilisation quotidienne.

---

## Index des fonctionnalités

**Capacités de l'agent**
- [Vraie automatisation de navigateur (CDP / Chromium)](#vraie-automatisation-de-navigateur-cdp--chromium)
- [Lecture de PDF : texte numérique + pages scannées via la vision](#lecture-de-pdf--texte-numérique--pages-scannées-via-la-vision)
- [Fichier → contexte de chat : glisser-déposer, clic droit, collage](#fichier--contexte-de-chat-glisser-déposer-clic-droit-collage)
- [Citer une réponse dans le rédacteur](#citer-une-réponse-dans-le-rédacteur)
- [Correctif de fiabilité Gemini / Antigravity](#correctif-de-fiabilité-gemini--antigravity)
- [Suivi automatique des modèles par compte](#suivi-automatique-des-modèles-par-compte)
- [Permissions de l'agent](#permissions-de-lagent)
- [Commandes slash et invocation à la demande](#commandes-slash-et-invocation-à-la-demande)
- [Diagnostics des serveurs de langage](#diagnostics-des-serveurs-de-langage)
- [Mémoire persistante](#mémoire-persistante)

**Espace de travail & UX**
- [Prise en charge du shell WSL / Ubuntu](#prise-en-charge-du-shell-wsl--ubuntu)
- [Le panneau Git passe par WSL sur les espaces de travail WSL](#le-panneau-git-passe-par-wsl-sur-les-espaces-de-travail-wsl)
- [Préférence de shell Auto intelligente](#préférence-de-shell-auto-intelligente)
- [Agent de migration Windows → WSL](#agent-de-migration-windows--wsl)
- [Import MCP depuis Claude Code / Codex](#import-mcp-depuis-claude-code--codex)
- [Sélecteur de modèle pour l'agent de migration](#sélecteur-de-modèle-pour-lagent-de-migration)
- [Quick Open (Ctrl+P) et recherche dans le chat (Ctrl+F)](#quick-open-ctrlp-et-recherche-dans-le-chat-ctrlf)
- [Notifications de workflow](#notifications-de-workflow)
- [Relais mobile à distance : auto-hébergement, questions, push et changement de projet](#relais-mobile-à-distance-auto-hébergement-questions-push-et-changement-de-projet)
- [Mise en page : panneaux repliables, fenêtres Réglages/Remote séparées](#mise-en-page-panneaux-repliables-fenêtres-réglagesremote-séparées)
- [UX de chat : bulles iMessage + copie des blocs de code](#ux-de-chat-bulles-imessage--copie-des-blocs-de-code)
- [Compétences : regroupement thématique, accordéon, « Tout activer / Tout désactiver »](#compétences-regroupement-thématique-accordéon-tout-activer--tout-désactiver)
- [Terminal : Ctrl+C copie / Ctrl+V colle intelligents](#terminal-ctrlc-copie--ctrlv-colle-intelligents)
- [Écran d'accueil : sélecteur de shell, entrée Réglages, entrée migration](#écran-daccueil-sélecteur-de-shell-entrée-réglages-entrée-migration)
- [Récents : épingler & supprimer](#récents-épingler--supprimer)
- [Carte de capacités « Quoi de neuf » sur l'accueil](#carte-de-capacités-quoi-de-neuf-sur-laccueil)
- [Ouverture maximisée + centrée sous Windows](#ouverture-maximisée--centrée-sous-windows)

---

### Vraie automatisation de navigateur (CDP / Chromium)

L'agent peut piloter un vrai navigateur Chromium via le protocole Chrome
DevTools — ouvrir des pages, faire des captures d'écran, lire le DOM,
cliquer, saisir du texte, évaluer du JS, etc. — pour qu'il puisse
réellement *regarder* un site en cours d'exécution au lieu de deviner à
partir du code source. Cherry-pick et adaptation des travaux de Glamgar sur
[Paseru/sinew#28](https://github.com/Paseru/sinew/pull/28).

- Nouvelle crate `crates/sinew-browser/` (chromiumoxide 0.7 + tokio) :
  gestion de session, DOM, captures d'écran, enregistrement GIF.
- 18 outils d'agent dans `crates/sinew-app/src/browser.rs`
  (`browser_open`, `browser_screenshot`, `browser_dom`, `browser_click`,
  `browser_eval`, …), noms dans `tool_names.rs` (constantes `BROWSER_*`).
- Câblés dans le tour dans `crates/sinew-app/src/agent/turn.rs` : les
  descripteurs de navigateur sont ajoutés **uniquement quand
  `browser_enabled` et pas en mode Plan**, plus un bloc de prompt système
  `<browser_tools>` pour que l'agent sache que le navigateur existe
  (sinon il tente de faire `apt-get install chromium` dans le shell).
- **Chrome est préféré à Edge** dans
  `session.rs::find_browser_executable` (recherche aussi dans les chemins
  `%LOCALAPPDATA%` de l'utilisateur).
- **Avec interface par défaut** : le builder de chromiumoxide injecte
  `--headless`, donc nous appelons `.with_head()` explicitement quand
  `headless = false` — vous *voyez* Chrome naviguer.
- **Bascule navigateur forcée** (bouton globe dans le rédacteur) : mode à
  usage unique qui ajoute une instruction forçant l'agent à utiliser les
  outils `browser_*` au lieu de lire le code.
  `src/components/chat/ChatPane.tsx`.
- **Captures d'écran intégrées** : les cartes d'outil `browser_*`
  affichent la capture d'écran renvoyée directement sous la carte
  (`src/components/chat/ToolCard.tsx`).

Nécessite Chrome ou Edge installé sur la machine hôte (non inclus).

### Lecture de PDF : texte numérique + pages scannées via la vision

L'outil `read` sait maintenant lire les PDF. Un routeur choisit le bon
chemin selon le document, pour que le cas courant reste gratuit/local et
que seules les numérisations paient un traitement plus lourd :

- **PDF numérique** (contient une couche texte, ~90 % des PDF) →
  `liteparse` extrait le texte en **markdown**, lu par plages de lignes
  comme n'importe quel fichier texte. Gratuit, entièrement local,
  ~3 ms/page, exact.
- **PDF scanné** (couche texte vide/clairsemée, heuristique <50 caractères
  par page) → `liteparse` rend les pages en PNG via PDFium et **les joint
  en tant qu'images**, pour que le modèle de vision de l'agent
  (Gemini / Claude / GPT) les lise. Pas de moteur OCR, aucune
  configuration — le LLM est l'OCR. Limité à 10 pages.

Notes :
- `liteparse` est ajouté avec `default-features = false` pour **supprimer
  la fonctionnalité `tesseract`** — le Tesseract intégré nécessite CMake
  pour compiler leptonica/tesseract à partir des sources C++ (échoue sur
  un Windows propre) et nous n'en voulons pas de toute façon. Le résultat
  de l'outil indique à l'agent de *ne pas* faire appel à un OCR externe
  via le shell.
- Un backend **Mistral OCR** plus précis (clé API optionnelle) est la
  prochaine couche prévue — purement additif.
- **Réserve d'emballage** : PDFium est fourni sous forme de `pdfium.dll`
  (~6,7 Mo) chargée à l'exécution ; le MSI doit l'inclure à côté de
  l'exécutable (`liteparse-pdfium` cherche dans le répertoire
  `current_exe` en repli), sinon la prise en charge PDF casse sur les
  machines autres que celle de compilation.

Code : `crates/sinew-app/src/read.rs` (`read_pdf`, détection de scan,
`render_pdf_pages`), dépendance dans `crates/sinew-app/Cargo.toml`. Les
lectures de PDF s'affichent comme une carte d'outil dépliable normale pour
que vous puissiez voir le markdown extrait / les vignettes de pages
(`src/components/chat/ToolCard.tsx`).

### Fichier → contexte de chat : glisser-déposer, clic droit, collage

Faites entrer des fichiers dans le contexte de l'agent sans taper de
chemins :

- **Glissez** une entrée de l'arborescence de fichiers vers le chat / le
  rédacteur → ajoutée comme contexte.
- **Clic droit → « Ajouter au chat »** sur un fichier.
- **Collez** dans l'arborescence de fichiers (`Ctrl+V` sur un dossier) :
  un fichier copié depuis l'Explorateur ou une **capture d'écran** du
  presse-papiers est importé dans l'espace de travail.

Le transfert entre composants utilise un singleton de module + un
événement CustomEvent `sinew:add-files-to-chat` sur la fenêtre. Les
fichiers du presse-papiers Windows sont lus via PowerShell
`Get-Clipboard -Format FileDropList` ; les captures d'écran via les blobs
image de `navigator.clipboard.read()` (le gestionnaire de `Ctrl+V` au
clavier appelle `preventDefault` sur le collage natif, donc le repli sur
les blobs est nécessaire).

Code : `src/components/FileTree.tsx` (glisser-déposer, menu contextuel,
`onPaste` / `importClipboardImages`), `src/components/chat/ChatPane.tsx`
(écouteur d'événement), `src-tauri/src/platform.rs` (Windows
`FileDropList`).

### Citer une réponse dans le rédacteur

Sélectionnez une partie d'un message de l'assistant → une petite pilule
**« Ajouter au contexte »** apparaît → cliquez dessus pour préfixer la
sélection en bloc de citation `>` dans le rédacteur, pour que votre tour
suivant puisse répondre à un fragment précis.

Protégé contre le `click` synthétique parasite qu'une petite sélection par
glissement déclenche (la pilule ignore les clics dans les 250 ms suivant
son apparition). Sélectionner du texte dans votre **propre** message ne
déclenche plus de rembobinage non plus (le gestionnaire de rembobinage
s'arrête quand il y a une sélection active).

Code : `src/components/chat/ChatPane.tsx` (`quoteAction`,
`applyQuoteToComposer`), `.quote-pill` dans `src/styles.css`.

### Correctif de fiabilité Gemini / Antigravity

Les modèles Google (endpoint Antigravity) renvoyaient une erreur 400
`INVALID_ARGUMENT` sur tout projet ayant déjà un historique d'outils — les
nouveaux projets fonctionnaient, les projets démarrés non. Deux causes
racines, toutes deux dans `crates/sinew-google/src/client.rs` :

- Chaque `functionResponse` était nommé `generic_tool` (le nom de l'outil
  ne pouvait pas être retrouvé à partir de l'identifiant d'appel) alors
  que le `functionCall` portait le vrai nom → Gemini fait correspondre les
  deux **par nom**, donc chaque paire ne correspondait pas. Corrigé en
  mappant `tool_call_id → nom` à partir des parties `ToolCall` et en
  étiquetant correctement les réponses.
- Les parties `thought` rejouées sans leur `thoughtSignature` d'origine
  étaient rejetées ; les pensées non signées sont désormais abandonnées au
  lieu d'être envoyées nues.

Un vidage du corps de requête vers `%TEMP%\sinew-google-bad-request.json`
sur toute erreur 400 a été ajouté pour rendre le diagnostic futur trivial.

### Mémoire persistante

L'agent retient d'une conversation à l'autre ce que le code ne montre pas :
tes préférences, tes corrections, les décisions et les pièges du projet.

| Portée | Contenu typique | Fichier |
|---|---|---|
| **Toi** (tous les projets) | « réponds en français », « jamais de push sans demander » | `~/.sinew/memory/user.md` |
| **Projet** | « la release se fait en poussant un tag vX.Y.Z » | `~/.sinew/memory/projects/<projet>.md` |

- Les fichiers sont **hors du dépôt** (privés, jamais commités par erreur).
  Markdown simple, une entrée `- [id] texte` par ligne, modifiable à la
  main. `SINEW_MEMORY_DIR` permet de les déplacer.
- Les deux portées sont ajoutées au prompt système au début de chaque tour
  (environ 8 Ko chacune au plus) : une modification s'applique au tour
  suivant et le cache de prompt tient pendant le tour.
- L'outil `memory` (`add` / `update` / `remove` / `list`) permet à
  l'agent principal de les modifier ; les agents secondaires et le swarm ne
  font que les lire. Tout ce qui ressemble à un secret (clé API, token, mot
  de passe, clé privée) est refusé.
- `/remember <texte>` demande à l'agent d'enregistrer quelque chose.
- Chaque écriture affiche une carte « Saved about you / for this project »
  avec **Undo**.
- Réglages → Outils → **Memory** : activé/désactivé, et « Agent decides » ou
  « Only when asked ». Réglages → **Memory** : liste, ajout, modification,
  suppression, ouverture des fichiers.
- **Tes instructions** : `~/.sinew/AGENTS.md` est chargé dans tous les
  projets. Un projet sans `AGENTS.md` mais avec un `CLAUDE.md` utilise
  celui-ci.

Code : `crates/sinew-app/src/memory.rs` (stockage, outil, section du
prompt), `agent/turn.rs` (injection), `global_instructions` /
`workspace_instructions` dans `src-tauri/src/turns.rs`, commandes
`memory_list` / `memory_apply` / `memory_file_path`, `MemoryCard` dans
`src/components/chat/ToolCard.tsx`, `MemorySection` dans
`src/components/SettingsPane.tsx`.

### Diagnostics des serveurs de langage

Après chaque `edit_file` / `write_file`, Sinew demande au serveur de langage
du projet si le fichier contient des erreurs et les ajoute au résultat de
l'outil : l'agent corrige ce qu'il vient de casser sans attendre un build
complet.

| Langage | Serveur | Installation |
|---|---|---|
| Rust | `rust-analyzer` | `rustup component add rust-analyzer` |
| TypeScript / JavaScript | `typescript-language-server` | `npm install -g typescript typescript-language-server` |
| Python | `pyright-langserver` | `npm install -g pyright` |
| Go | `gopls` | `go install golang.org/x/tools/gopls@latest` |

- Un serveur démarre à la première édition d'un fichier de son langage, s'il
  est installé (rien n'est installé automatiquement), et reste ouvert pour le
  projet. Dans un projet `\\wsl$\…`, il tourne dans WSL, et les chemins sont
  traduits entre les formes UNC et Linux.
- Une édition attend **3 s au plus**. Seules les **erreurs** sont ajoutées
  (20 au plus) ; si le serveur démarre ou indexe encore, le résultat le dit.
- Le nouvel outil `diagnostics` permet à l'agent de vérifier d'autres
  fichiers (`paths`) ou de lister tout ce que signalent les serveurs,
  avertissements compris.
- La carte d'édition affiche une pastille « N errors » qui déplie la liste ;
  la carte `diagnostics` affiche un badge.
- Réglages → Outils → **Diagnostics** : interrupteur (activé par défaut), état
  de chaque serveur (non installé / trouvé / indexation / actif / en échec)
  avec la commande d'installation, chemin de binaire personnalisé,
  « Restart servers ».

Code : `crates/sinew-app/src/lsp.rs` (client JSON-RPC, registre des serveurs,
traduction des chemins WSL, l'outil), branchements dans
`agent/tool_dispatch.rs` et `agent/turn.rs`, commandes `lsp_status` /
`lsp_restart`, `DiagnosticsStrip` dans `src/components/chat/ToolCard.tsx`.

### Commandes slash et invocation à la demande

Taper `/` dans le chat ouvre un menu avec trois familles :

- **Actions** exécutées par l'interface : `/help`, `/new`, `/compact`,
  `/plan`, `/act`, `/goal`, `/permissions`.
- **Commandes** = prompts envoyés à l'agent. Intégrées : `/review` (relit les
  changements non commités), `/commit`, `/push` (commite s'il y a des
  modifications puis pousse la branche, avec `-u origin` si besoin, jamais de
  push forcé), `/init` (crée/met à jour `AGENTS.md`), `/test`, `/remember`. Les tiennes :
  un fichier Markdown dans `.sinew/commands/` ou `.claude/commands/` (projet)
  ou dans les mêmes dossiers de ton home (Windows et WSL). Format compatible
  Claude Code : frontmatter `description:` / `argument-hint:`, `$ARGUMENTS`,
  `$1`…`$9`. Projet > global > intégrée.
- **Compétences**, y compris celles désactivées.

La bulle affiche ce que tu as tapé (`/review src/`) ; le prompt de la
commande part dans une partie cachée du message. L'expansion se fait côté
Rust, donc les commandes marchent aussi depuis le téléphone.

`@` propose maintenant les **serveurs MCP** (au-dessus des fichiers).

**Invocation à la demande** : une compétence ou un serveur MCP désactivé
reste invisible pour l'agent, mais si tu le nommes dans un message
(`/frontend-design`, `@github`), il est activé pour **toute la suite de cette
conversation** (et seulement elle). L'agent ne peut donc pas s'en servir de
lui-même sans que tu l'aies invoqué.

Réglages → **Commands** liste les commandes avec leur source et crée une
nouvelle commande dans `.sinew/commands/`.

Code : `crates/sinew-app/src/commands.rs` (découverte, intégrées, expansion,
`apply_invocations`), `send_message` dans `src-tauri/src/turns.rs`, commandes
`list_slash_commands` / `create_slash_command` / `list_mcp_mentions`, menus
dans `src/components/chat/ChatPane.tsx`.

### Permissions de l'agent

L'agent ne peut plus tout exécuter sans garde-fou. Chaque appel d'outil passe
par `crates/sinew-app/src/permissions.rs` avant de s'exécuter, avec cet ordre
de priorité :

1. **Liste noire, toujours active** : les commandes qui peuvent détruire la
   machine (`rm -rf /`, `~`, `/mnt/c`, `Remove-Item -Recurse C:\`, `format C:`,
   `mkfs`, `dd of=/dev/sda`, bombe fork…) sont refusées, même à travers `sudo`,
   `bash -c`, `wsl --`, `cmd /c`, `$(…)` ou un enchaînement `; && |`. Aucune
   règle ni aucun mode ne les débloque.
2. Règles `deny`, puis `ask`, puis `allow` (globales dans Réglages → Outils →
   Permissions, plus `.sinew/permissions.json` par projet). Format :
   `bash: npm run *`, `edit: src/**`, `mcp: mcp__github__*`.
3. **Commandes risquées, toujours demandées** : `git push --force`,
   `git reset --hard`, `git clean -f`, `rm -rf` hors du projet, `curl … | sh`,
   `npm publish`, `DROP TABLE/DATABASE`.
4. **Mode** : *Auto* (défaut), *Demander pour le shell*, *Tout demander*
   (shell + éditions + MCP). Le bouton bouclier de la barre du chat change de
   mode d'un clic.

Une demande affiche une carte dans le chat : *Allow once*, *Always allow in
this project* (enregistre la règle dans `.sinew/permissions.json`), *Deny* ou
*Deny with a message* (le message est transmis à l'agent). Notification si la
fenêtre n'est pas au premier plan, push et boutons sur le téléphone via
Remote. Les demandes des sous-agents remontent dans le chat principal ; Stop
refuse la demande en attente. Les cartes d'outil affichent `BLOCKED` /
`DENIED` quand un appel n'a pas été exécuté.

C'est un garde-fou contre les accidents, pas un bac à sable : une commande
volontairement camouflée peut passer.

Code : `permissions.rs` (règles + tests), `agent/tool_dispatch.rs`
(`permission_gate`), `agent/cancel.rs` (`ApprovalReply`), commande
`answer_approval` dans `src-tauri/src/turns.rs`,
`src/components/chat/ApprovalCard.tsx`.

### Suivi automatique des modèles par compte

Le sélecteur de modèles n'est plus seulement une liste figée dans le code :
Sinew demande à chaque provider connecté quels modèles **votre compte** peut
réellement utiliser, et avec quelles limites.

- Anthropic → `GET /v1/models`, OpenAI (ChatGPT) → endpoint Codex `/models`
  (clé API → `/v1/models` filtré), Google → Antigravity
  `fetchAvailableModels`, Kimi Code → `/models`, DeepSeek → `/models`.
- Un provider qui répond **remplace** ses entrées statiques : les modèles
  connus gardent leur libellé et leurs niveaux de réflexion, les nouveaux
  apparaissent automatiquement (libellé et niveaux déduits de la liste). Un
  provider hors ligne ou non connecté garde le catalogue statique.
- Les fenêtres de contexte réelles remplacent les valeurs codées en dur
  (ex. GPT-6 Luna via ChatGPT : 272k et non 1,05M), et Antigravity fournit
  l'id exact à envoyer (`gemini-3.8-flash-tiered`).
- Résultat mis en cache 6 h côté Rust ; un échec ponctuel garde la dernière
  liste valide. La version client Codex annoncée suit la dernière release
  npm de `@openai/codex`, comme pour Claude Code.

Code : `crates/sinew-core/src/discovered.rs` (registre + surcharge des
capacités), `crates/sinew-*/src/discovery.rs` (une liste par provider),
commande `list_discovered_models` dans `src-tauri/src/providers.rs`, fusion
dans `src/lib/models.ts` (`catalogWithDiscovered`).

### Prise en charge du shell WSL / Ubuntu

L'outil `bash` de l'agent et le terminal interactif peuvent désormais
passer par `wsl.exe`. Choisissez votre shell dans **Réglages → Outils →
Terminal et shell** : `Auto`, `PowerShell` ou `WSL`.

Sous le capot :

- `crates/sinew-app/src/bash.rs` — nouvelle variante `ShellKind::Wsl`,
  `path_targets_wsl_filesystem()`, caches atomiques pour le type de shell
  actif et la préférence globale.
- `src-tauri/src/terminal.rs` — `default_terminal_command()` choisit le
  bon shell ; pour WSL, nous retirons le préfixe verbatim `\\?\UNC\` du
  répertoire de travail de l'espace de travail avant de le passer à
  `wsl.exe` (il ne peut pas traduire les chemins verbatim).
- Distribution WSL par défaut + shell de connexion, pour que votre
  NVM / asdf / alias se chargent comme dans une session `wsl` normale.

### Le panneau Git passe par WSL sur les espaces de travail WSL

Sinew d'origine délègue au `git.exe` natif de Windows pour le panneau Git.
Sur les espaces de travail `\\wsl$\…`, cela déclenche la vérification
« dubious ownership » (CVE-2022-24765) à chaque appel (UID Linux ≠
utilisateur Windows), donc le panneau affiche **« Not a Git repository »**
pour un dépôt parfaitement valide. Chaque utilisateur devrait alors lancer
manuellement :

```powershell
git config --global --add safe.directory '%(prefix)///wsl$/Ubuntu/home/<vous>/projects/<dépôt>'
```

…avant que le panneau ne fonctionne.

Ce fork contourne tout le problème : à la frontière `run_checked` /
`run_output` dans `src-tauri/src/git.rs`, toute invocation git sur un
espace de travail que `sinew_app::path_targets_wsl_filesystem` signale est
routée via `wsl.exe -- git <args>` avec le chemin de l'espace de travail
comme répertoire de travail. `wsl.exe` traduit automatiquement le cwd en
chemin Linux natif, git s'exécute dans Linux, voit un dépôt correspondant
à l'UID et fonctionne normalement — aucune configuration utilisateur,
aucun relâchement de sécurité.

Les espaces de travail `C:\` / `D:\` utilisent toujours le `git.exe`
natif — seuls les chemins WSL passent par `wsl.exe`. Cela reprend le
modèle du shell intelligent.

### Préférence de shell Auto intelligente

`Auto` est le réglage par défaut des nouvelles installations. Quand il est
actif :

- Le chemin de l'espace de travail commence par `\\wsl$\…` /
  `\\wsl.localhost\…` → **WSL**.
- Tout le reste (`C:\`, `D:\`, …) → **PowerShell**.
- macOS / Linux → inchangé, toujours bash.

Vous n'avez plus à basculer le shell global quand vous passez d'un projet
natif Windows à un projet WSL. Le terminal interactif et l'outil `bash` de
l'agent résolvent `Auto` de façon cohérente — même shell, même répertoire
de travail.

La préférence peut être changée depuis deux endroits : le sélecteur en
ligne de l'écran d'accueil (rail droit) ou **Réglages → Outils → Terminal
et shell**. Les deux surfaces exposent les trois mêmes options explicites
(`Auto` / `PowerShell` / `WSL`) pour qu'il n'y ait aucune ambiguïté sur le
sens de chacune.

Code : `terminal.rs::resolve_shell_preference()`,
`bash.rs::set_active_shell_for_workspace()`,
`workspace.rs::open_workspace()` (crochet à chaque ouverture d'espace de
travail).

### Agent de migration Windows → WSL

Deux points d'entrée :

- **Écran d'accueil** : un CTA secondaire *« Migrer un projet Windows vers
  WSL »*.
- **Barre latérale** : une petite icône à côté du nom de l'espace de
  travail, visible uniquement quand l'espace de travail ouvert est un
  chemin Windows.

Le déroulement :

1. Choisissez le dossier source (chemin Windows) — sélecteur de fichiers
   ou pré-rempli depuis la barre latérale.
2. Choisissez la cible (par défaut
   `\\wsl$\Ubuntu\home\<vous>\projects\<basename>`, modifiable).
3. Sinew crée le dossier cible si besoin, refuse d'écraser une cible non
   vide sans accusé de réception explicite, et l'ouvre comme espace de
   travail.
4. Une nouvelle conversation démarre en mode Objectif (Goal), pré-remplie
   avec un prompt de migration structuré. Il ne reste qu'à appuyer sur
   Envoyer.

L'agent ensuite :

- inspecte la source (détection de stack),
- vérifie l'état git, **utilise l'outil `question`** pour décider entre
  re-clone (propre + avec remote) et copie de fichiers (sale ou sans
  remote),
- copie via `rsync -a` avec des exclusions judicieuses
  (`node_modules`, `target`, `dist`, `.venv`, …),
- met en évidence les variables d'environnement / particularités Windows /
  problèmes de chemin Python `.venv`,
- termine par une checklist markdown des étapes post-migration.

Code : `src-tauri/src/workspace.rs::prepare_migration_target`,
`crates/sinew-app/src/store.rs::MIGRATION_AGENT_PROMPT`,
`src/components/MigrationDialog.tsx`,
`src/lib/pendingMigration.ts` (transfert entre composants).

### Import MCP depuis Claude Code / Codex

L'onglet MCP démarrait vide à chaque nouvelle installation. Si vous aviez
déjà utilisé Claude Code ou Codex CLI, vous aviez une liste parfaitement
valable de serveurs MCP configurés là-bas et vous deviez retaper chacun
d'eux pour les utiliser dans Sinew.

Un bouton **Importer…** dans **Réglages → Serveurs MCP** ouvre maintenant
un sélecteur de fichiers sur `.json` / `.toml`, détecte automatiquement le
format d'après l'extension et fusionne les serveurs dans les réglages de
Sinew (les doublons par nom sont ignorés, donc relancer l'opération est
sans risque) :

- **Claude Code / Claude Desktop** — choisissez `~/.claude.json`
  (Windows : `C:\Users\<vous>\.claude.json` ; WSL :
  `\\wsl$\Ubuntu\home\<vous>\.claude.json`). Lit l'objet `mcpServers` de
  premier niveau.
- **Codex CLI** — choisissez `~/.codex/config.toml`. Lit les sections
  `[mcp_servers.<nom>]` (les anciennes `[mcp.<nom>]` sont aussi
  acceptées).

La sonde MCP se relance automatiquement après l'import pour que les
serveurs nouvellement ajoutés s'allument en vert/rouge immédiatement.

Code : `crates/sinew-app/src/mcp.rs::{parse_mcp_import_file,
merge_imported_mcp_servers}`, commande Tauri `import_mcp_servers_command`
dans `src-tauri/src/conversations.rs`, interface dans
`src/components/SettingsPane.tsx::McpSection`.

### Sélecteur de modèle pour l'agent de migration

La MigrationDialog vous laisse choisir quel LLM exécute la migration avant
de lancer. La liste affiche tous les modèles dont le fournisseur est
configuré ; le choix transite par le transfert de migration et est
persisté comme **modèle du mode Objectif** de la nouvelle conversation
avant l'envoi.

Code : `src/components/MigrationDialog.tsx` +
`src/components/chat/ChatPane.tsx` (le gestionnaire de pré-remplissage
utilise une ref vers `persistModeSelection` pour rester stable tout en
voyant le dernier callback).

### Quick Open (Ctrl+P) et recherche dans le chat (Ctrl+F)

- **Ctrl+P** — palette de fichiers floue avec support du suffixe `:line`
  et récents sauvegardés dans localStorage. Correspondance floue
  personnalisée sans dépendance (`src/lib/fuzzy.ts`) avec chemin rapide
  par sous-chaîne, bonus de début de segment / série / nom de fin. UI :
  `src/components/QuickOpen.tsx`.
- **Ctrl+F (dans le chat)** — recherche dans le message via l'API CSS
  Custom Highlight (`CSS.highlights`, `::highlight()`). Pas de mutation du
  DOM React, pas de tempête de re-rendu. Hook + composant :
  `src/components/chat/useChatSearch.ts`,
  `src/components/chat/ChatSearch.tsx`.

### Notifications de workflow

Quand la fenêtre Sinew n'est pas focalisée, soyez notifié quand l'agent :

- termine un tour (transition `status` de running → idle),
- pose une question (l'outil `question` se déclenche),
- finalise un plan (mode Plan → `planReady`),
- termine une exécution en mode Objectif
  (`goalWorkflow.status === "complete"`).

Notification au niveau du système d'exploitation via
`tauri-plugin-notification`, plus clignotement de la barre des tâches via
`request_user_attention`. Activable/désactivable dans **Réglages → Outils →
Notifications**.

Aide : `src/lib/notify.ts` (`notify`, `pingUserAttention`,
`isWindowFocused`, `flashTaskbar` — tout en best-effort, erreurs avalées).
Les déclencheurs vivent dans `src/components/chat/ChatPane.tsx`.

### Relais mobile à distance : auto-hébergement, questions, push et changement de projet

Sinew Remote permet à un téléphone apparié de piloter l'agent qui tourne
sur votre PC. C'est une **fonction de contrôle à distance, pas un agent
cloud** : votre PC doit rester en ligne, car il exécute les modèles, les
outils, les commandes shell, l'automatisation du navigateur et les fichiers
de projet. Le téléphone et le PC chiffrent toutes les commandes et tous les
événements de bout en bout ; le relais ne fait qu'acheminer des paquets
chiffrés opaques.

Le panneau Remote contient un guide de configuration concis. Pour un guide
complet et copiable qu'un LLM peut suivre, voir
[`remote/README.md`](remote/README.md). Il couvre :

- le déploiement du relais indépendant `remote/` sur Railway (Root
  Directory : `remote`, un seul réplica, `PORT` fourni par Railway) ;
- la génération de clés VAPID pour le Web Push optionnel des
  questions/fins de tâche ;
- le réglage de l'URL du relais du bureau à `wss://<votre-domaine>/ws` ;
- l'appairage d'un téléphone, l'installation de la PWA et l'activation des
  notifications ;
- la révocation sûre d'un appareil perdu ou remplacé.

Mobile Remote prend en charge le streaming en direct, les questions en mode
Plan avec relecture à la reconnexion, **Stop** pour les tours actifs,
l'approbation des plans, les notifications quand une question attend une
réponse, et le changement/ouverture des projets que le PC a déjà ouverts.
Un téléphone ne peut ouvrir qu'un projet récent enregistré ; les chemins
arbitraires envoyés par un client sont rejetés.

**Sécurité :** ne commitez et ne partagez jamais les clés privées VAPID,
les codes d'appairage, les jetons d'appareil, les clés API ou les fichiers
`.env`. Révoquez immédiatement un téléphone apparié s'il est perdu. Un
appareil apparié peut commander l'agent sur votre PC, donc n'appariez que
des appareils que vous contrôlez.

### Mise en page : panneaux repliables, fenêtres Réglages/Remote séparées

- La barre latérale (fichiers) et le panneau de chat ont tous deux des
  rails de repli — une fine barre verticale sur laquelle cliquer pour les
  replier.
- Le panneau central d'édition est masqué automatiquement quand aucun
  fichier/terminal n'est ouvert. Dès que vous ouvrez un fichier ou
  invoquez un terminal, il revient en glissant.
- **Réglages** et **Remote** s'ouvrent dans leurs propres fenêtres Tauri
  dédiées (routage d'URL `?view=settings|remote`). Auparavant, ils
  s'affichaient en ligne dans le panneau central et se retrouvaient
  cachés derrière un gros panneau de chat. Maintenant chacun vit dans sa
  propre fenêtre native avec la chrome OS normale.

Code : `src-tauri/src/platform.rs::create_secondary_window`,
`src/components/Workspace.tsx` (le drapeau `editorShellVisible` et les
gestionnaires `openSettings`/`openRemote`).

### UX de chat : bulles iMessage + boutons de copie

- Les messages utilisateur s'alignent à droite avec un léger fond de
  bulle ; les messages de l'assistant restent alignés à gauche et sans
  encadrement (fini l'accent de rail — upstream avait une rayure violette
  que j'ai retirée).
- Les blocs de code dans les messages de chat reçoivent un bouton
  **Copier** révélé au survol, comme dans Claude Code lui-même.
- **Tout le message de l'assistant** reçoit aussi un bouton de copie
  révélé au survol dans le coin supérieur droit — pratique pour récupérer
  la réponse markdown sans la sélectionner manuellement. Composant :
  `src/components/chat/MessageCopyButton.tsx`.
- Les blocs de code sont enveloppés/surlignés via `rehype-highlight`.

### Compétences : regroupement thématique, accordéon, « Tout activer / Tout désactiver »

Le panneau des compétences était une liste plate. Maintenant :

- Les compétences sont regroupées dans des catégories thématiques :
  **Frontend & Design** (fusion des Design et Frontend séparés
  d'upstream), **IA & Agents**, **Backend**, **Sécurité**, **DevOps**,
  etc.
- Chaque catégorie est un accordéon repliable (fermé par défaut).
- Une bascule unique **Tout activer / Tout désactiver** par catégorie,
  plus une bascule globale en haut.

Les règles vivent dans `src/components/SettingsPane.tsx`. L'ordre compte :
les catégories spécifiques (Design / Frontend) sont appariées avant le
fourre-tout IA pour que des éléments comme `design-taste-frontend`
atterrissent dans le bon seau.

### Terminal : Ctrl+C copie / Ctrl+V colle intelligents

- **Ctrl+C** quand il y a une sélection → copie et efface la sélection.
  Pas de sélection → envoie `^C` au shell comme d'habitude.
- **Ctrl+V** → colle le presse-papiers dans le flux d'entrée du PTY.

Implémenté dans la liaison shell xterm.js. Évite le piège classique du
terminal Windows « j'ai voulu copier mais j'ai tué le processus ».

### Écran d'accueil : sélecteur de shell, entrée Réglages, entrée migration

Un utilisateur qui ouvre Sinew pour la première fois doit savoir comment le
câbler avant même de toucher un projet. La page d'accueil fait ressortir
trois choses :

- Un petit **engrenage** dans le coin supérieur gauche (loin du bouton de
  fermeture) ouvre les Réglages dans une fenêtre séparée. Permet de
  configurer les clés API / fournisseurs avant d'ouvrir un espace de
  travail.
- Un **sélecteur de shell de terminal** (Windows uniquement) dans le rail
  droit avec trois options explicites — **Auto** / **PowerShell** / **WSL**
  — et une astuce qui explique, en français courant, ce que fait chaque
  mode (y compris le piège « WSL sur /mnt/c est lent »). Par défaut sur
  Auto.
- Un CTA **Migrer vers WSL** dans le même rail. Clic → MigrationDialog.

Toute la page tient maintenant sur un écran : titre + Ouvrir un dossier +
récents dans la colonne principale, les deux actions secondaires dans un
rail droit collant (scène de 880 px sous Windows, colonne unique de 520 px
ailleurs).

Code : `src/components/Welcome.tsx` +
`src/lib/ipc.ts::openSecondaryWindow` / `getShellPreference` /
`setShellPreference`. Backend : commandes Tauri `get_shell_preference` /
`set_shell_preference` dans `src-tauri/src/conversations.rs` —
indépendantes de l'espace de travail pour que le sélecteur soit utilisable
avant l'ouverture d'un dossier.

### Récents : épingler & supprimer

Survoler un espace de travail récent sur la page d'accueil révèle deux
actions : une **épingle** (📌) qui le remonte en haut et l'y maintient
quelle que soit sa récence, et une **suppression** (✕). Les entrées
épinglées ne comptent pas dans le plafond des récents, donc elles restent
collantes. Persistées dans la même liste localStorage avec un champ
supplémentaire `pinned?: boolean`.

Code : `src/lib/recents.ts` (`removeRecent`, `toggleRecentPinned`,
`compareRecents`), `src/components/Welcome.tsx`, `src/types.ts`.

### Carte de capacités « Quoi de neuf » sur l'accueil

Certaines fonctionnalités phares de ce fork (lecture PDF, automatisation
de navigateur, citation dans le contexte) ne sont pas des fonctions IDE
évidentes, donc une carte désactivable **« Quoi de neuf / Ce que cet agent
sait faire »** sur la page d'accueil les met en avant une fois — sur
l'écran d'atterrissage, pour ne jamais encombrer l'IDE pendant le travail.
La désactivation est persistée (clé localStorage
`sinew.whatsNewDismissed.v1`) ; incrémentez le suffixe de la clé pour la
faire réapparaître après une future mise à jour.

Code : `src/components/Welcome.tsx` (`CAPABILITIES`), styles
`.welcome__whatsnew*` dans `src/styles.css`.

### Ouverture maximisée + centrée sous Windows

Sinew se lance **maximisé et centré** par défaut sous Windows au lieu de
s'ouvrir à une taille arbitraire à moitié hors écran — corrige le problème
de « la fenêtre apparaît en dehors du bord de l'écran ».

Code : `src-tauri/src/lib.rs` (crochet de création de fenêtre).

---

## Compilation / installation

### Télécharger le MSI (utilisateurs finaux)

`.msi` pré-compilé pour Windows dans la section
[Releases](../../releases). Lancez-le, démarrez Sinew, configurez un
fournisseur de modèle dans les Réglages, terminé.

### Compiler depuis les sources (forkeurs)

Prérequis (Windows) :

- **Rust** via `rustup` (utilisez `winget install Rustlang.Rustup` puis
  `rustup default stable`)
- **Visual Studio Build Tools 2026** avec la charge de travail *Développement
  Desktop en C++* (ou VS 2022 — les deux fonctionnent)
- **Node.js LTS** + npm (`winget install OpenJS.NodeJS.LTS`)
- Pour que les fonctionnalités WSL soient utiles : **WSL2 + une
  distribution Ubuntu** (`wsl --install -d Ubuntu`)

Ensuite :

```powershell
git clone https://github.com/<votre-nom-utilisateur>/sinew.git
cd sinew
git checkout feat/wsl-shell
npm install
npm run tauri dev          # build de dev avec rechargement à chaud
npm run tauri build        # produit target\release\bundle\msi\*.msi
```

Le `.msi` n'est signé que si vous configurez des identités de signature
dans `tauri.conf.json` ; le build non signé s'installe quand même, Windows
affiche simplement un avertissement SmartScreen la première fois.

### Configuration WSL utile pour l'agent

Si vous voulez que l'outil `bash` de l'agent trouve les outils Node dans
WSL, installez Node via `nvm` et assurez-vous qu'il se charge
automatiquement dans `~/.profile` (pas seulement `~/.bashrc`) — Sinew
lance `wsl.exe -- bash -lc <commande>`, qui charge `.profile`.

```bash
# dans WSL Ubuntu
curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh | bash
# nvm s'installe dans .bashrc, mais ajoutez-le aussi dans .profile :
echo '[ -s "$NVM_DIR/nvm.sh" ] && \. "$NVM_DIR/nvm.sh"' >> ~/.profile
nvm install --lts
```

---

## Synchronisation avec upstream

La branche du fork est `feat/wsl-shell`, basée sur `Paseru/main`. Pour
tirer les modifications d'upstream :

```bash
git remote add upstream https://github.com/Paseru/sinew.git    # une fois
git fetch upstream
git checkout feat/wsl-shell
git rebase upstream/main
# résolvez les conflits (généralement dans src/components/Workspace.tsx et
# les fichiers liés aux Réglages, car ce sont les plus modifiés ici)
```

La frontière des crates Rust (`crates/sinew-app/src/bash.rs`, `store.rs`)
et le côté `src-tauri/` restent étroitement ciblés ; les conflits y sont
rares. Les conflits frontend se résolvent la plupart du temps proprement en
préférant « ours » pour les sections mise en page / Welcome / Réglages et
« theirs » pour tout le reste.

---

## Par où commencer pour personnaliser

Si vous voulez étendre ce fork davantage :

| Domaine | Commencez ici |
|---|---|
| Nouvelle variante de shell | `crates/sinew-app/src/bash.rs::ShellKind`, plus la branche correspondante dans `src-tauri/src/terminal.rs::default_terminal_command` |
| Une autre fenêtre secondaire (ex. « Stats ») | `src-tauri/src/platform.rs::create_secondary_window` + paramètre de vue URL dans `src/main.tsx` |
| Un nouveau déclencheur de notification | `src/components/chat/ChatPane.tsx` — cherchez `pingUserAttention` |
| Une autre palette d'actions rapides | clonez le modèle `QuickOpen.tsx` (modale + `fuzzy.ts`) |
| Un autre flux d'agent (comme celui de migration) | le modèle vit dans `crates/sinew-app/src/store.rs` en `pub const`, plus un petit module singleton `pendingX` + événement de fenêtre pour le transfert entre composants |
| Un nouveau CTA d'accueil | `src/components/Welcome.tsx` — les boutons secondaires suivent le même modèle `welcome__cta--secondary` |

---

## Licence

Hérite de la licence de Paseru/sinew (voir `LICENSE`). Ce fork ne revendique
aucun droit supplémentaire — c'est juste ma couche de personnalisation
personnelle.
