# HEIC Viewer

Visionneuse de photos HEIC/HEIF (iPhone) pour Windows **sans** les extensions payantes
du Microsoft Store (*HEIF Image Extensions* / *HEVC Video Extensions*) : le décodage
HEVC est fait par le crate [`heic`](https://crates.io/crates/heic), écrit à 100 % en Rust.
Résultat : un seul `.exe` autonome, sans DLL ni installation.

Ouvre aussi les AVIF (décodés par le même crate, sans l'extension *AV1 Video Extension*), JPEG,
PNG, GIF, BMP, WebP et TIFF, pour parcourir un dossier mélangé.

## Compiler

```sh
./build.sh                # -> dist/heic-viewer.exe  (Windows x64, cross-compilé)
./build.sh linux          # -> dist/heic-viewer      (Linux)
./build.sh test           # tests
./build.sh cargo clippy   # n'importe quelle commande cargo
```

- **Avec Docker** (utilisé s'il est installé) : rien d'autre à installer. Le `Dockerfile` part
  de `rust:latest` et utilise MinGW ; le cache cargo est dans le volume `heic-viewer-cargo`.
- **Sans Docker** : il faut [rustup](https://rustup.rs) avec la cible
  `x86_64-pc-windows-gnu`, [zig](https://ziglang.org) et
  [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild) dans le `PATH` ; zig sert
  d'éditeur de liens pour Windows.

Les tests comprennent le suivi réel d'un dossier temporaire (renommage, modification,
suppression signalés par le système). Test du décodage d'une vraie photo :
`HEIC_SAMPLE=/chemin/photo.heic ./build.sh test` (avec Docker, le projet est monté dans `/src` :
`HEIC_SAMPLE=/src/photo.heic`).

`cargo deny check` (avec [cargo-deny](https://github.com/EmbarkStudios/cargo-deny), réglé par
`deny.toml`) vérifie les dépendances : licences compatibles avec l'AGPL 3.0, failles connues
(base RustSec), provenance (crates.io uniquement). Une alerte est écartée, avec sa raison dans
`deny.toml` : `paste`, plus maintenu mais sans faille connue, ne sert qu'à la compilation du
décodeur AV1.

Inutile de compiler avec `-C target-cpu=x86-64-v3` : les crates `heic` et `moxcms` détectent
AVX2 à l'exécution et l'utilisent déjà (aucun gain mesuré), alors que l'option rendrait l'exe
incompatible avec les processeurs sans AVX2.

## Intégration continue et versions

Sur GitHub, `.github/workflows/ci.yml` vérifie chaque envoi et chaque demande de fusion (format,
clippy pour Linux et Windows, tests, `cargo deny`), puis compile les deux exécutables avec zig,
téléchargeables dans les artefacts de l'exécution. Pour publier une version, au choix :

- **à la demande** : onglet *Actions*, workflow *CI*, *Run workflow*, en indiquant le numéro
  (`1.2.3`, ou `1.2.3-beta.1` pour une préversion) ; l'étiquette `v1.2.3` est créée sur le
  commit compilé ;
- **par une étiquette** : `git tag v1.2.3 && git push origin v1.2.3`.

Le numéro donné remplace celui de `Cargo.toml` dans les exécutables (fenêtre *À propos*) ; celui
du dépôt ne sert qu'aux compilations locales. La version GitHub créée contient
`heic-viewer-windows-x86_64.exe`, `heic-viewer-linux-x86_64` (à rendre exécutable :
`chmod +x`) et leurs sommes de contrôle (`SHA256SUMS`), avec des notes tirées des commits.
L'exécutable Linux ne demande que la glibc 2.17 (2012) : il démarre sur toutes les
distributions actuelles, et même bien plus anciennes.

## Licence

[AGPL 3.0](LICENSE), imposée par le crate `heic` (sous AGPL 3.0 ou licence commerciale) : le
code source doit rester accessible à qui reçoit l'exécutable, ce que fait la publication sur
GitHub. Les autres dépendances ont des licences compatibles (MIT, Apache 2.0, BSD…), vérifiées
par `cargo deny`. Les polices d'egui intégrées (Hack, Ubuntu, Noto Emoji…) gardent leurs
licences libres (OFL, Ubuntu Font Licence…). Le HEVC peut être couvert par des brevets (à
garder en tête en cas de diffusion).

## Utilisation

- Ouvrir : bouton **Ouvrir…** / `Ctrl+O`, clic sur la zone vide, ou en
  argument (`heic-viewer.exe photo.heic`). Sous Windows : clic droit sur un `.heic` →
  *Ouvrir avec* → *Choisir une autre application* → `heic-viewer.exe` (cocher « Toujours »).
- **Glisser-déposer** un fichier (ou un dossier) dans la fenêtre : l'image s'affiche, la fenêtre
  prend le focus et le contexte précédent/suivant devient le dossier de cette image.
- **Nouvelle instance** (Windows) : clic molette sur le bouton de la visionneuse dans la barre
  des tâches, comme pour les autres applications.
- **Dossier suivi en temps réel**, même quand la visionneuse n'a pas le focus : une image
  supprimée (ou déplacée) quitte la liste, une image ajoutée ou restaurée depuis la Corbeille y
  entre, une image renommée garde sa place dans l'ordre sous son nouveau nom, une image modifiée
  est décodée de nouveau. Si l'image affichée disparaît, la suivante la remplace.

| Action | Clavier | Souris |
|---|---|---|
| Image précédente / suivante | `←` `→` (hors zoom), `PgPrec` `PgSuiv`, `Espace`, `Retour arrière` | clic gauche sur la moitié gauche / droite (le curseur devient une flèche), boutons latéraux précédent / suivant |
| Première / dernière | `Début` / `Fin` | |
| Zoom | `+` / `-` | molette (centré sur le curseur) |
| Taille réelle ×1 à ×9 | `1` à `9` (même touche une 2ᵉ fois : retour à la vue d'avant) | |
| Ajuster à la fenêtre | `0` / `Z` | |
| Déplacer l'image zoomée | `←` `↑` `→` `↓` | glisser (clic gauche) |
| Menu contextuel | | clic droit |
| Copier l'image (ou le texte sélectionné dans les propriétés) | `Ctrl+C` | |
| Afficher dans le dossier | `W` | |
| Renommer | `F2` | |
| Mettre à la Corbeille (Windows) | `Suppr` | |
| Propriétés de l'image | `Alt+Entrée` | |
| Pivoter à droite / à gauche | `R` / `L` (`Maj` inverse le sens) | |
| Plein écran | `F11`, `F`, `Entrée` | clic molette |
| Quitter | `Q`, `Échap` (hors plein écran) | |

`Échap` ferme d'abord ce qui est ouvert (menu, propriétés, plein écran), puis quitte ; pendant
un glisser-déposer, il annule seulement le glisser.
En plein écran, un bandeau rappelle brièvement la position, le nom et le zoom à chaque
changement d'image ou de zoom, et reste affiché pendant un chargement. En sortant du plein
écran, la fenêtre retrouve exactement sa position et sa taille, y compris dans sa zone
d'ancrage (Windows Snap) ; Windows ne la considère alors plus comme ancrée (il ne permet pas de
rétablir cet état).
Les chiffres fonctionnent sans `Maj` sur un clavier AZERTY (touche physique).

- **Renommer** (`F2`) : le nom est proposé avec sa partie avant l'extension sélectionnée ; un
  nom déjà pris ou invalide est signalé sans rien changer.
- **Mettre à la Corbeille** (`Suppr`, Windows) : après confirmation, le fichier va dans la
  Corbeille, d'où il peut être restauré (il revient alors dans la liste) ; s'il ne peut pas y
  aller (lecteur réseau…), Windows avertit avant de le supprimer définitivement. L'image
  suivante s'affiche.
- **Afficher dans le dossier** : si un onglet de l'Explorateur montre déjà le dossier, il est
  activé et sa fenêtre passe au premier plan ; le fichier y est sélectionné et la liste défile
  jusqu'à lui. Sinon une nouvelle fenêtre s'ouvre.
- **Propriétés de l'image** : nom, taille, dates, dimensions, profil couleur et métadonnées
  EXIF (date de prise de vue, appareil, exposition, GPS…), lisibles aussi pour les HEIC et les
  AVIF. « Voir sur la carte » ouvre OpenStreetMap à l'endroit de la prise de vue. Un clic sur
  une valeur puis `Ctrl+C` copie cette valeur entière.
- **Propriétés du fichier** (Windows) : la boîte de dialogue de l'Explorateur.
- **Options** (bouton ⚙ de la barre d'outils, ou menu contextuel) : langue de l'interface
  (automatique : celle du système, français ou anglais, et l'anglais pour toute autre langue).
  Enregistrée dans `%APPDATA%\heic-viewer\config.toml` (Windows) ou
  `~/.config/heic-viewer/config.toml` ; mode portable : un fichier `heic-viewer.toml` à côté de
  l'exécutable est utilisé à la place s'il existe. **À propos** (bouton ℹ) : version et licence.
  Ces fenêtres se valident avec `Entrée` et se ferment avec `Échap`.

Les couleurs sont gérées : le profil ICC intégré (Display P3 des iPhone, Adobe RGB…) est
converti en sRGB ; sans ça les photos paraissent ternes. Au-delà de 100 %, les pixels sont
affichés nets, sans lissage. En dessous, dès que le zoom ne bouge plus (≈ 0,15 s), l'image est
recalculée en Lanczos3 aux dimensions exactes de l'écran, plus nette que la réduction de la
carte graphique utilisée pendant qu'on zoome. En passant d'une image à l'autre, ce calcul est
fait dès le décodage : l'image s'affiche nette d'emblée.

L'orientation EXIF/HEIF des photos est appliquée automatiquement ; les images voisines
sont décodées en arrière-plan pour une navigation fluide. Aucune image n'est jamais sautée :
avec des appuis rapides ou une touche maintenue, les images défilent une à une, chacune
visible au moins 0,06 s (16 par seconde au plus, un peu moins en pratique : la durée
s'arrondit au rafraîchissement de l'écran) ; une image lourde ralentit un peu le défilement,
le temps d'être décodée. Un appui isolé agit immédiatement ; des appuis rapides
ne s'accumulent pas : à l'arrêt, seule l'image demandée par le dernier appui s'affiche encore.
Pendant un défilement, deux images d'avance sont préchargées dans son sens. Une rotation
maintenue fait un quart de tour toutes les 0,06 s. Après un saut (Début / Fin), les décodages
devenus inutiles sont abandonnés.
