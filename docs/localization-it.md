# LightCraft in italiano

Scegli **Modifica → Lingua → Italiano** oppure **Impostazioni → Generali → Lingua**.
La scelta vale subito e viene salvata per il prossimo avvio (`language: "it"` in `ui.json`).

I cataloghi italiani coprono tutte le chiavi delle traduzioni esistenti e tutte le
diciture di comandi e cursori: menu, scorciatoie, gestione delle foto, modifica,
ritaglio, maschere, persone, importazione/esportazione, impostazioni, preimpostazioni
e profili integrati, cronologia, versioni, intestazioni di data e messaggi di stato.
Sono tradotti anche l'editor di regole degli album smart, la guida dei tag dei modelli
e la finestra delle modifiche non salvate, così come le note di versione incorporate.
Gli errori tecnici delle sezioni più interne restano in inglese, come nelle altre lingue.

Nomi di file, ID dei comandi, tag dei modelli e nomi assegnati dall'utente ad album,
preimpostazioni o metadati non vengono tradotti. I font Inter già inclusi contengono
i caratteri italiani; non serve alcun download aggiuntivo. Le descrizioni per SAM 3
vengono passate al modello senza modifiche; gli esempi usano i suoi termini inglesi.

## Manutenzione e verifica

Testi fissi: `crates/ui-egui/locales/it.json`. Messaggi con valori:
`crates/ui-egui/locales/it-formats.json`. Le chiavi inglesi restano invariate;
i segnaposto vengono verificati dal build di Rust. Le desinenze plurali inglesi
che non si applicano all'italiano vengono consumate senza stampa con `{:.0}`;
le parole invarianti come «foto» e «file» restano corrette al singolare e al plurale.
La data appare ad esempio come `domenica 20.09.2026`.

```sh
cargo test -p lightcraft-ui-egui i18n::tests
LIGHTCRAFT_LANGUAGE=it lightcraft-cli snapshot --demo -o italiano.png --size 1600x1000
```

I test verificano la copertura dei cataloghi, i segnaposto, il cambio di lingua da menu
e canale di controllo, il salvataggio della scelta, i formati di data, la conservazione
dei nomi utente e il rendering dei testi con gli accenti italiani. Un test di resa
controlla anche l'ampiezza dei pulsanti; la riga delle lingue nelle Impostazioni va a
capo quando servono più pulsanti di quanti ne contiene la finestra. Le lingue accettate
comprendono `it`, `it-IT`, `it_CH.UTF-8` e `it-SM`; viene sempre salvato `it`.
Ulteriori indicazioni in [localization.md](localization.md).
