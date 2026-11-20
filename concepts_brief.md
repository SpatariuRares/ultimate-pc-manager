# Package Manager (add-on)

* **Author:** spatariu.rares
* **Date:** 2026-11-17

## Overview

Oggi, dopo aver reinstallato Windows, l'autore deve aprire il sito di ogni programma e installarlo a mano. L'add-on, ispirato a UniGetUI, permette di cercare e installare software da winget (e in futuro da altre fonti facoltative), di aggiornare i pacchetti e di salvare le proprie app in un **bundle**: un file che sul PC nuovo si apre a mano per reinstallare tutto in serie. Ogni persona ha il proprio bundle.

L'add-on non lancia installazioni da solo al primo avvio di Windows: è l'utente che apre il bundle e preme Installa. UAC è accettato; serve la rete.

## Analysis

Sintesi di sette revisori indipendenti (design, dettagli del codice, semplicità, comprensione, prontezza all'implementazione, struttura, contraddizioni). Percorsi verificati: il codice vive in `src/addons/installed_apps/`, `src/addons/drivers/`, `src/platform.rs`, `src/addons.rs`; la piattaforma add-on esiste già (`Addon`, `ProcessRunner`, `AddonHost::for_tests`, `Job`/`Generation`, `uninterruptible()`, `TIERS`).

### Contraddizioni e rischi (in ordine di gravità)

1. **Sovrapposizione con Installed Apps e installazioni concorrenti (bloccante).** Installed Apps ha già il controllo e l'aggiornamento in serie, il riepilogo, i codici di winget tradotti in parole e il pannello dell'output. Ogni add-on blocca solo i propri lavori: due add-on che installano insieme falliscono con winget 0x8A150102 o MSI 1618 ("un'altra installazione è in corso"). Serve **un unico lucchetto o coda di installazione nella piattaforma**, condiviso da Installed Apps, Drivers e Package Manager, e va deciso chi possiede "aggiorna".
2. **La chiamata a winget non basta (bloccante).** `ProcessRunner::winget()` usa `Command::output()`: aspetta la fine senza avanzamento, timeout né stop, e ogni nuovo strumento (Scoop, Chocolatey) richiederebbe un nuovo metodo della piattaforma. Il runner di test che registra le chiamate restituirebbe sempre errore, quindi riepilogo e "continua dopo un errore" non sarebbero testabili. Serve una chiamata generica che legga l'output riga per riga e si possa fermare, più un runner finto che restituisce esiti preparati. È una modifica al PRD della piattaforma.
3. **Chiudere l'app a metà serie (bloccante).** Il thread muore: le installazioni successive non partono (come voluto), ma il riepilogo in memoria va perso. Non si sa se winget si interrompe quando la pipe che legge l'app si chiude: va provato a mano. Proposta: output di winget su file, un **giornale dell'esecuzione** su disco (stato, codice, log per pacchetto), al riavvio "Ultima esecuzione interrotta: 4 di 9 fatte, continuare?", conferma alla chiusura durante un'esecuzione.
4. **Disinstallazione: "come Installed Apps" e "per fonte" si contraddicono (bloccante).** Installed Apps disinstalla tramite registro, `MsiExec /X` o MSIX. Scoop, Chocolatey e i pacchetti portabili di winget spesso non sono nel registro. Proposta: ogni fonte disinstalla ciò che gestisce; il disinstallatore registro/MSI/MSIX passa nel codice di piattaforma e fa da riserva. Un add-on non deve chiamare il codice di un altro.
5. **Serie lunga e UAC.** Un bundle di 20–30 app può chiedere UAC per ogni installatore in momenti casuali, e la serie si ferma se non c'è nessuno al PC. Proposta: eseguire per primi gli installatori che chiedono i diritti di amministratore, così le richieste arrivano tutte nel primo minuto; usare `--silent --accept-package-agreements --accept-source-agreements --disable-interactivity`. Lanciare tutto come amministratore romperebbe le installazioni per utente e dello Store e la regola "mai avviare l'app come amministratore".
6. **Su Windows appena installato winget è al suo peggio.** App Installer può non essere registrato o essere vecchio, le versioni LTSC non hanno winget, gli accordi delle fonti non sono accettati, il primo aggiornamento delle fonti è lento. Serve un controllo di prontezza (manca / non registrato / troppo vecchio / pronto) con un'azione di riparazione. Va anche deciso come arriva Ultimate PC Manager sul PC nuovo (es. exe portabile con il bundle accanto).
7. **Leggere la tabella di testo di winget per la ricerca è fragile.** `parse_winget_table` è nata per `list`; `search` aggiunge la colonna Match e può troncare gli id con "…": un id troncato passato a `install --id --exact` fallisce o installa il pacchetto sbagliato. Caratteri larghi spostano le colonne; l'output italiano richiede esempi di test propri. Alternative: scartare o riinterrogare le righe troncate, il modulo PowerShell `Microsoft.WinGet.Client` (JSON) o l'API COM (come UniGetUI). Per elenco e bundle, `winget export` dà JSON.
8. **Un bundle è input non fidato che gira dietro UAC.** Un id come `--override ...` o argomenti liberi sono una via d'iniezione. Le voci sono solo dati `{source, id, version?, scope?}`, con caratteri ammessi limitati, id passati come argomento separato, solo fonti attive su questo PC, e sorgente e opzioni di ogni voce mostrate prima di partire.
9. **Il formato del bundle è un contratto che dura anni.** `winget export`/`import` esiste già ed è usabile anche senza l'app. Proposta: un JSON nostro con versione (`schema_version`, chiave `source`+`id`, campi sconosciuti conservati, una voce sbagliata non rifiuta il file), con import/export verso il formato di winget ed eventualmente `.ubundle` di UniGetUI. Le app che nessuna fonte conosce (proprio quelle "da sito") restano nel bundle come voci "da installare a mano" con il link al sito.
10. **Un'interfaccia comune per molte fonti è prematura.** Oggi c'è una sola fonte reale, e lo Store passa già da winget (`--source msstore`). Da subito il modello identifica un pacchetto come `(fonte, id)`; l'interfaccia con capacità dichiarate (`needs_admin`, `supports_user_scope`, …) e un controllo `availability()` va provata sulla carta contro Scoop, senza costruire Scoop.
11. **"Già installato".** Una lettura prima della serie segna come saltate le app presenti; il codice "già installato" conta come saltato, non come errore. Dopo ogni installazione, verifica con `winget list --id <id> --exact`; "installato ma non trovato" è un esito a sé.
12. **Codici d'uscita.** `winget_error` copre 9 codici (due etichette forse sbagliate: 0x8A150061, 0x8A150010). Copiare la tabella completa di `returnCodes.md` per una versione fissata e dividere in installato / saltato / fallito / serve riavvio / annullato (0x8A150109 e 3010 = installato con nota). Tenere il codice grezzo accanto alle parole; "non avviato" distinto da "eseguito e fallito".
13. **Riferimenti da dichiarare.** Il PRD deve dichiarare i riferimenti: UniGetUI per il comportamento, il design di Installed Apps per l'aspetto, un prototipo approvato prima (come per Power & Freq). Storie etichettate "(da UniGetUI)" / "(nostra aggiunta)".
14. **Dettagli.** "Fonte" significa già Registro/MSI/MSIX in Installed Apps: serve un'altra parola per l'utente. Un nuovo add-on tocca `TIERS` e i conteggi nei test di `src/addons.rs`. La finestra di salvataggio fa solo CSV. Ricerca su Invio (o con ritardo), mai automatica all'apertura. I `Requirements` sono fissi, ma i programmi necessari dipendono dalle fonti accese. Dopo un'installazione l'elenco di Installed Apps è vecchio: non c'è un evento tra add-on. "Package Manager" può suonare da sviluppatori.

### Schemi suggeriti

- **Estrarre prima il codice condiviso, senza cambi visibili**: `parse_winget_table`, `match_winget`, `winget_error`, `last_lines`/`is_progress`, `plural`/`join_names` e il disinstallatore registro/MSI/MSIX in un modulo di piattaforma; il meccanismo di lavoro in serie oggi copiato tra Installed Apps e Drivers in un unico esecutore in serie (stop dopo la voce corrente, voce non interrompibile, giornale, riepilogo) sopra `Job`/`Generation`. I test di Installed Apps devono restare verdi.
- **Coda di installazione nella piattaforma**: gli add-on inviano `{fonte, id, azione, scope}`; la coda ne esegue uno alla volta e mostra chi la tiene ("Installed Apps sta aggiornando 3 app").
- **Esecuzione del bundle come macchina a stati**: ogni voce è in attesa / in corso / ok / fallita(codice) / saltata / non avviata. Una sola struttura guida avanzamento, riepilogo e giornale; il testo del riepilogo è una funzione pura.
- **Log**: `--log <giornale>\<esecuzione>\<id>.log`, ogni errore collegato al suo log, "Apri cartella dei log".
- **Divisione dei compiti**: Installed Apps risponde a "cosa c'è sul PC ed è aggiornato?" (aggiorna, disinstalla); Package Manager a "cosa posso installare e qual è la mia lista portabile?".

### Ideal Final Result

Sul PC vecchio un clic salva le app in un bundle che è solo dati (e, dove possibile, anche un file valido per `winget import`). Sul Windows nuovo si avvia l'exe portabile, si apre il bundle: l'add-on verifica (e ripara) winget, mostra le app già spuntate e da dove arrivano; quelle sconosciute compaiono con il link al sito. Si preme Installa, si rispondono le richieste UAC nel primo minuto e ci si allontana. winget fa il lavoro; l'add-on è un selettore, una coda ordinata, un giornale e un traduttore di esiti, costruito su servizi di piattaforma condivisi. Al ritorno un solo riepilogo in parole (installate, saltate e perché, fallite e perché, serve riavvio), ogni errore con il suo log e "Riprova le fallite". Se l'app si chiude, va in crash o il PC si riavvia, al prossimo avvio si vede dove si era fermata e si può continuare.

## Goals vs. Non-Goals

* **In-Scope Goals (v1):**
  - winget come unica fonte (lo Store tramite winget); il modello usa già `(fonte, id)`.
  - Cercare e installare un pacchetto.
  - Creare un bundle dalle app installate; aprire un bundle a mano, scegliere, installare in serie, continuare dopo un errore, riepilogo in parole.
  - Giornale dell'esecuzione e proposta di ripresa dopo la chiusura.
  - Controllo di prontezza di winget.
  - Lucchetto di installazione nella piattaforma.
  - Estrazione del codice condiviso come primo passo.
* **Out-of-Scope Non-Goals:**
  - Scoop, Chocolatey e altre fonti (ognuna con un suo PRD breve più avanti).
  - Una seconda schermata di aggiornamento o disinstallazione.
  - Argomenti di installazione liberi nei bundle; installazioni in parallelo; helper elevato; API COM; associazione del file bundle; ripresa automatica.
  - Impostazioni di Windows, debloat, driver (altri add-on), installazione automatica al primo avvio, Linux e Mac.

## Alternatives Weighed

- **Usare UniGetUI così com'è**: esiste ed è gratuito. Scartato dall'autore: l'obiettivo è avere tutto in un'unica app con add-on (direzione PowerToys).
- **Copiare UniGetUI per intero (tutte le fonti in v1)**: scartato dalla revisione per costo; le fonti oltre winget arrivano una alla volta.
- **Installazione automatica e senza intervento al primo avvio di Windows**: scartata dall'autore; l'utente apre il bundle a mano.
- **Fondere Package Manager e Installed Apps in un solo add-on**: scartato dall'autore; restano due add-on che condividono il codice. (La revisione segnala che la versione minima, "Esporta le mie app" + "Installa da bundle" dentro Installed Apps, coprirebbe da sola la motivazione.)
- **Eseguire tutta la serie come amministratore**: sconsigliato dalla revisione; romperebbe le installazioni per utente e dello Store e la regola "mai avviare come amministratore".
- **Impostazioni di Windows, debloat, driver nello stesso add-on**: scartati dall'autore; saranno altri add-on.

## Key Decisions & Staging for Next Steps

**Già deciso dall'autore**
- Add-on separato da Installed Apps, con codice condiviso.
- Cerca, installa, aggiorna, disinstalla (come Installed Apps), salva bundle.
- Bundle aperto a mano su un PC nuovo; niente installazione automatica.
- Più fonti, tutte facoltative; UAC accettato; serve la rete.

**Decisioni aperte da chiudere prima del PRD**
1. Chi possiede "aggiorna": Package Manager, Installed Apps o entrambi tramite il codice condiviso?
2. Disinstallazione: lo strumento della fonte o registro/MSI/MSIX, e chi ha la precedenza?
3. UAC: una richiesta per installatore, con prima quelli da amministratore?
4. Formato del bundle: JSON nostro con versione e adattatori per winget/`.ubundle`, o direttamente quello di winget? Versioni fissate o sempre l'ultima? App sconosciute tenute come link al sito?
5. Come leggere winget: tabella con protezione dagli id troncati, modulo PowerShell o COM?
6. Chiusura durante un'esecuzione: conferma? Giornale e ripresa al prossimo avvio?
7. Lucchetto/coda di installazione e chiamata generica al runner nella piattaforma (modifiche al PRD della piattaforma).
8. Versione minima di winget e riparazione su Windows appena installato.
9. "Già installato": saltare o lasciare che winget aggiorni?
10. Accordi dei pacchetti: accettati in silenzio o chiesti all'utente?
11. Scope predefinito: utente o macchina.
12. Più fonti: impegno fermo (interfaccia in v1) o "forse" (solo `(fonte, id)` nel modello)?
13. Riferimento di design e prototipo da approvare prima.
14. Tier (gratis o a pagamento) e nomi per l'utente: nome dell'add-on e parola al posto di "fonte".

**Ordine suggerito**: A. estrazione nella piattaforma → B. fonte winget, ricerca e installazione → C. bundle, giornale, riepilogo → D. una fonte in più per PRD breve. Un solo PRD, `docs/prd/package-manager.md`, per A–C, con le sezioni "Alternative considerate" e "Rischi e mitigazioni", più un piccolo emendamento a `installed-apps.md`.
