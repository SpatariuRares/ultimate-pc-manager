# Lavoro futuro: idee dalle app di riferimento

Raccolta del 2026-11-17. Direzione del prodotto: **un PC manager, "le impostazioni fatte bene"**.
Le app elencate sono in `Desktop\utility` (più Microsoft PC Manager, installato dallo Store).
Sono idee da valutare, non decisioni: core o add-on, priorità e prezzo vanno ancora decisi
(vedi `docs/addons.md`).

Escluse perché non interessano: FrameView, Everything.

## 1. Pagine esistenti: riferimenti da confrontare

| App di riferimento | Cosa prendere | Pagina |
|---|---|---|
| BCUninstaller (open source) | Disinstallazione di più app insieme, ricerca dei residui (registro e cartelle), app orfane, disinstallazione silenziosa | Installed Apps |
| Revo Uninstaller | Ricerca dei residui dopo la disinstallazione, con livelli sicura, moderata e avanzata | Installed Apps |
| TreeSize Free | Velocità di scansione, totali, viste per tipo di file e per età | Disk Space |
| DDU (Display Driver Uninstaller) | Rimozione completa dei driver GPU e audio, avvio in modalità provvisoria, blocco dei driver di Windows Update | Drivers |
| NVIDIA App | Controllo e installazione degli aggiornamenti dei driver GPU | Drivers |
| EaseUS Partition Master | Mappa delle partizioni, stato del disco (SMART) | Disk Space o add-on "Disks" |
| Armoury Crate / iCUE / G HUB | Curve delle ventole, profili di prestazioni, sensori | Power & Freq e i sensori |

## 2. Nuovi add-on possibili, in ordine di priorità

1. **Cleanup**: pulizia approfondita come il Deep cleanup di PC Manager (cache di Windows, log, file
   temporanei, cestino, cache delle app) e pulizia automatica programmata come Storage Sense, più le
   cache e gli installer dei launcher di giochi presi da SteamCleaner (Steam, Epic, Origin…).
   Va affiancato a Disk Space.
2. **Health check / Riparazioni**: una pagina che controlla lo stato del PC e corregge con un clic:
   ripristino della rete (da collegare a Connections), riparazione della barra delle applicazioni, app
   predefinite e browser predefinito, controllo di Windows Update.
3. **File duplicati**, e una vista dei Download, dentro Disk Space.
4. **Tweaks / Privacy** sul modello di Winhance (open source): rimozione delle app preinstallate
   (debloat), impostazioni di privacy e telemetria, ottimizzazioni che si possono annullare.
5. **Boost** (liberare RAM e chiudere i processi in background con un clic): ultimo. Windows libera
   già la memoria da solo quando serve, quindi serve più a mostrare qualcosa che a fare davvero.

Bassa priorità:
- Blocco dei pop-up (da PC Manager).
- Toolbox e barra sul desktop con scorciatoie (da PC Manager).
- Segnalare le voci sospette in Startup apps e Services (idea presa da Malwarebytes e Bitdefender,
  senza copiare l'antivirus).
- Verificare che Connections mostri bene gli adattatori VPN (NordVPN, Bitdefender VPN).

## 3. Piattaforma add-on

- **Windhawk**: modello di riferimento per il catalogo degli add-on (si attivano e disattivano, ogni
  add-on ha le sue impostazioni).

## Microsoft PC Manager e la nostra app

Confronto basato su recensioni in rete: il pacchetto installato non è stato aperto. Da verificare
con gli screenshot delle sezioni.

| Funzione di PC Manager | Da noi |
|---|---|
| Boost con un clic | Manca |
| Health check | Manca (Summary mostra lo stato ma non propone correzioni) |
| Deep cleanup | Manca |
| File grandi / duplicati / scaricati | In parte: Disk Space trova i file grandi; mancano i duplicati e la vista dei Download |
| Storage Sense | Manca |
| Gestione delle app | Installed Apps (più completo) |
| Gestione dei processi | Processes (più completo) |
| App all'avvio | Startup apps |
| Windows Update, scansione con Defender | Mancano |
| Protezione delle app predefinite, blocco dei pop-up | Mancano |
| Riparazioni rapide (barra delle applicazioni, rete, Edge) | Mancano |
| Toolbox e barra sul desktop | Manca |

Cosa ha la nostra app e PC Manager no: Performance, Services, Drivers, Power & Freq, Flight Recorder,
Connections, App history, Users, Benchmarks, System Info.

Fonti: [wtop (gennaio 2026)](https://wtop.com/tech/2026/01/data-doctors-should-i-be-using-microsoft-pc-manager),
[Windows Report](https://windowsreport.com/microsoft-pc-manager-review/),
[BetaNews](https://betanews.com/article/microsoft-pc-manager-feels-like-ccleaners-little-brother),
[lazyadmin.nl](https://lazyadmin.nl/it/microsoft-pc-manager/).
