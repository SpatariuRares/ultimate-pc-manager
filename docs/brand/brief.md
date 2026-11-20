# Ultimate PC Manager — Brief per una nuova identità visiva

Documento da dare a ChatGPT (o a chi disegna) per creare **da zero** l'identità
del prodotto: icona dell'app, simbolo, logo con scritta e palette.

**Regola principale:** non è un restyling. Niente linea a battito cardiaco, niente
"MY TM", niente riferimenti all'aspetto attuale dell'app o ai task manager
esistenti. L'interfaccia verrà adattata al nuovo marchio, non il contrario.

---

## 1. L'idea in una riga

> **L'ultimo task manager di cui avrai bisogno.**
> Tutto il PC (processi, hardware, servizi, driver, app, disco, rete,
> benchmark) in un'unica cabina di comando.

"Ultimate" non vuol dire "un altro task manager": vuol dire **il livello più
alto**. È lo strumento definitivo che sostituisce il Gestione attività di Windows
e una mezza dozzina di utility.

---

## 2. Il prodotto

**Ultimate PC Manager** è un'app desktop nativa per **Windows 10/11** (scritta in
Rust): leggera, veloce, senza installazioni pesanti, senza abbonamenti né
funzioni "Pro" bloccate.

Cosa riunisce in un solo programma:

| Area | Cosa fa |
|---|---|
| **Monitoraggio live** | CPU core per core, memoria, dischi, rete, GPU, consumi in watt, temperature, frequenze |
| **Processi** | vedere tutto ciò che gira, chiuderlo, cambiarne la priorità; utenti e storico delle app |
| **Registrazione** | un "registratore di volo" che salva le metriche nel tempo per capire cosa è successo |
| **Sistema** | servizi, programmi all'avvio, driver (con aggiornamenti da Windows Update) |
| **Software** | elenco delle app installate e aggiornamento in un clic |
| **Rete** | tutte le connessioni attive |
| **Disco** | cosa occupa lo spazio, con storico delle scansioni |
| **Benchmark** | test di CPU, disco, GPU e connessione Internet, con storico dei risultati |
| **Info** | scheda completa dell'hardware |

---

## 3. Personalità del marchio

- **Pubblico:** power user, gamer, appassionati di hardware, tecnici, chi vuole
  sapere *esattamente* cosa fa il proprio PC.
- **Come deve sentirsi chi lo vede:** *"questo è lo strumento serio, quello
  definitivo"*. Sicurezza, padronanza, precisione.
- **Tre aggettivi:** **definitivo · preciso · potente**.
- **Anche:** essenziale (un'unica app al posto di tante), affidabile, rapido.
- **Non deve essere:** giocoso, cartoon, "gaming RGB" esagerato, burocratico o da
  software aziendale anni 2000, una copia di icone esistenti.

### Territori di significato (da cui pescare)
- **Vetta / apice:** il punto più alto, "ultimate".
- **Centro di comando:** controllo totale, tutto converge in un punto.
- **Unificazione:** molti strumenti diventano uno solo.
- **Precisione:** misura, calibrazione, ingegneria.
- **Nucleo:** il cuore del sistema, ciò che sta sotto tutto.

---

## 4. Direzioni creative da esplorare

Fai sviluppare a GPT **almeno 3 direzioni diverse**, ciascuna con 3–4 varianti.

### A. Apex
Una forma a vetta o chevron verso l'alto, netta e geometrica, costruita da
livelli o segmenti sovrapposti. Racconta "il livello più alto" e un insieme di
strati (hardware, sistema, app) che culmina in un punto.

### B. Command Core
Un nucleo centrale circondato da anelli o settori concentrici, come un reattore
o un mirino di precisione. Racconta il controllo totale: tutto ruota attorno a
un centro che lo governa.

### C. Unified U
Un monogramma **U** costruito da moduli o blocchi che si incastrano, a indicare
molti strumenti fusi in uno. Deve funzionare come lettera e come simbolo
astratto.

### D. Ultimate Mark
Un simbolo astratto e unico, come un sigillo o un emblema geometrico (esagono,
diamante tagliato, stella a pochi lati), che comunica "massimo grado" senza
illustrare nulla di letterale.

### Da evitare in tutte le direzioni
- Linee a battito o elettrocardiogramma, grafici a linea, tachimetri: sono il
  cliché dei system monitor.
- Chip con i piedini disegnati, ingranaggi, chiavi inglesi, scudi antivirus.
- Somiglianze con le icone del Gestione attività di Microsoft o di altri task
  manager.
- Loghi o colori riconoscibili di Windows, Intel, AMD, NVIDIA.
- Corone e trofei: sono troppo letterali per "ultimate".

---

## 5. Colore e tipografia (liberi)

La palette è **da inventare**: non bisogna riusare i blu attuali dell'app.
Chiedi a GPT di proporre 3 palette, ognuna con:
- un **colore principale** (l'accento del marchio, che diventerà anche il colore
  dei pulsanti e della selezione nell'app);
- un **colore di fondo scuro** e uno **chiaro** (l'app ha il tema scuro e quello
  chiaro);
- **1–2 colori secondari**;
- i codici **hex**.

Spunti di partenza, solo come esempio:
- **Ultra Violet:** viola elettrico `#7b5cff` + magenta, su nero bluastro.
- **Molten:** arancio fuso `#ff6a1a` + oro, su grafite.
- **Ion:** verde acido `#b6ff3b` + bianco, su nero puro.

**Tipografia:** un sans-serif geometrico e deciso per la scritta
"Ultimate PC Manager". Se il font deve finire dentro l'app, deve avere una
**licenza libera** (SIL OFL, come quelli di Google Fonts). Chiedi a GPT di indicare
il nome del font usato.

---

## 6. Asset da consegnare

Metti i file in `assets/brand/` del repository oppure caricali nella chat con
Claude.

### Obbligatori

| File | Formato | Misura | Uso |
|---|---|---|---|
| `icon-1024.png` | PNG, sfondo **trasparente** | 1024 × 1024 | icona dell'app: finestra, barra delle applicazioni, `.exe`, menu Start |
| `mark.png` | PNG, sfondo trasparente | 512 × 512 | solo il simbolo, senza piastra di fondo, per la barra laterale dell'app |
| `palette.md` (o testo in chat) | testo | — | colori hex scelti e nome del font |

### Consigliati

| File | Formato | Misura | Uso |
|---|---|---|---|
| `icon-small.png` | PNG trasparente | 256 × 256 | versione semplificata per 16–32 px |
| `logo-dark.png` | PNG trasparente | 1200 × 300 | simbolo + scritta, su fondo scuro |
| `logo-light.png` | PNG trasparente | 1200 × 300 | simbolo + scritta, su fondo chiaro |
| `icon.svg`, `mark.svg` | SVG | — | versioni vettoriali, se disponibili |
| `banner.png` | PNG | 1280 × 640 | anteprima del repository GitHub e delle release |

### Requisiti tecnici dell'icona
- Deve essere riconoscibile anche a **16 × 16 px** (barra delle applicazioni,
  Gestione attività di Windows): **una sola forma forte**.
- Deve funzionare su barra delle applicazioni **scura e chiara**.
- Stile coerente con **Windows 11**: forme pulite, angoli morbidi, eventuale
  leggero gradiente, niente 3D realistico.
- La forma occupa circa l'**85–90 %** del quadrato ed è centrata.
- **Nessun testo dentro l'icona.**

> **Consiglio:** GPT sbaglia spesso le lettere. Per `logo-dark` e `logo-light`
> va bene anche consegnare solo `mark.png` più il nome del font: la scritta la
> compone l'app, ed è più nitida.

---

## 7. Prompt pronti per GPT

**1) Esplorazione (prima chat)**
```
I'm creating a brand identity from scratch for "Ultimate PC Manager", a native Windows 10/11 app that
replaces the built-in Task Manager and a handful of utilities: live CPU/GPU/memory/disk/network monitoring,
processes, services, drivers, installed-app updates, disk space analysis, benchmarks and a "flight recorder"
of system metrics. Audience: power users, gamers, hardware enthusiasts. Personality: definitive, precise,
powerful — "the last task manager you'll ever need". Avoid: heartbeat/ECG lines, line charts, speedometers,
chips with pins, gears, shields, crowns, anything resembling Microsoft Task Manager or hardware brands.
Propose 4 distinct logo directions (e.g. an apex/peak made of stacked layers, a command core with concentric
rings, a modular "U" monogram, an abstract emblem), each with a short rationale, and 3 original color palettes
with hex codes plus a free (SIL OFL) geometric sans-serif font for the wordmark.
```

**2) Icona (dopo aver scelto la direzione)**
```
Create the app icon for "Ultimate PC Manager" following direction [NOME DIREZIONE] and palette [HEX].
Single bold geometric symbol on a rounded-square plate, modern Windows 11 app icon style, clean flat shapes
with a subtle gradient, strong silhouette that stays recognizable at 16x16 pixels, centered, filling ~88%
of the canvas. No text, no letters. Transparent background, 1024x1024 PNG.
```

**3) Simbolo senza piastra**
```
Same symbol as the icon, without the background plate, for use on a dark app sidebar and on a light one.
Transparent background, 512x512 PNG.
```

**4) Versione piccola**
```
Simplify this icon for 16x16 and 32x32 pixels: remove fine details, thicken strokes, keep only the main
silhouette, maximize contrast. Transparent background, 256x256 PNG.
```

**5) Logo con scritta**
```
Horizontal logo: the symbol on the left, the wordmark "Ultimate PC Manager" on the right in [FONT],
"Ultimate" emphasized. Two versions: for dark background and for light background.
Transparent background, 1200x300 PNG. Spell the text exactly: Ultimate PC Manager.
```

---

## 8. Checklist prima di consegnare

- [ ] È davvero nuova: nessun battito, grafico, tachimetro o chip.
- [ ] Ridotta a 16 × 16 e 32 × 32 si riconosce ancora.
- [ ] Funziona su fondo scuro e su fondo chiaro.
- [ ] Sfondo trasparente vero (non bianco e non a scacchi disegnati).
- [ ] Nessun testo nell'icona; nel logo la scritta "Ultimate PC Manager" è esatta.
- [ ] Palette e font indicati con hex e nome.
- [ ] Non somiglia a icone di altri task manager o a marchi hardware.

---

## 9. Cosa farà Claude dopo la consegna

1. Genera l'`.ico` multi-risoluzione (16–256) dall'icona.
2. Mette l'icona sulla finestra e sulla barra delle applicazioni (`src/main.rs`)
   e la incorpora nell'`.exe` con un `build.rs`.
3. Sostituisce il logo provvisorio "UPM" della barra laterale (`src/widgets.rs`,
   funzione `logo()`) con il nuovo simbolo e la scritta.
4. Porta la nuova palette nell'app: colore principale, selezione e accenti in
   `src/theme.rs`, per i temi scuro e chiaro. Se serve, aggiunge il nuovo font.
5. Fa gli screenshot nei due temi e controlla l'icona a 16 e 32 px.
6. Aggiorna lo zip della release e il README.
