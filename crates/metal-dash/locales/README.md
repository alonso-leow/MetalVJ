# UI languages

The shell is written in English. Every string it paints goes through
`shell::i18n::tr("…")` (`trf` when it has `{placeholders}`, `tr_dyn` when the
text comes from the door, e.g. a catalog label). This folder holds the other
languages: one folder per language, one TOML file per shell module, each a flat
map from the English source text to the translation.

```toml
# locales/es/lanes.toml
"save as sequence" = "guardar como secuencia"
"{n} files" = "{n} archivos"
```

- A missing entry falls back to English; it never paints a blank label.
- Placeholders (`{n}`) keep their names in every language and may move.
- The same English key in two files must carry the same translation
  (`i18n::tests` fails otherwise), and every entry must still be used by a
  `tr`/`trf` literal, except in `dynamic.toml`.
- The active language is `language` in `data/settings.json` (`"en"` or
  `"es"`), set from the settings overlay.

To add a language: a new `locales/<code>/` folder with the same files, a new
`Lang` variant and its row in `ES_FILES`' sibling table in `shell/i18n.rs`.

## Glossary

Use these words so every panel says the same thing. The Spanish column is what
the UI said before the English rewrite; keep it verbatim in `es/`.

| English | Spanish |
|---|---|
| properties | propiedades |
| node / nodes | nodo / nodos |
| lanes | lanes |
| library | biblioteca |
| grid (overlays grid) | grilla |
| mask | máscara |
| output | salida |
| screen | pantalla |
| sequence | secuencia |
| movement | movimiento |
| layer | capa |
| collection | colección |
| source | fuente |
| effect | efecto |
| switch | interruptor |
| node control | control de nodos |
| press / toggle (switch kinds) | presionar / booleano |
| input / output (ports) | entrada / salida |
| wired / not wired | cableado / sin cable |
| fixed / reactive | fijo / reactivo |
| on / off | encendido / apagado |
| train | entrenar |
| settings | ajustes |
| language | idioma |
| save / load | guardar / cargar |
| play / view | tocar / ver |
| edit / show (mode toggle) | editar / mostrar |
| add / create / remove / delete | añadir / crear / quitar / borrar |
| open monitor | abrir monitor |
| hand over / take back (a screen) | ceder / retomar |
| calibrate | calibrar |
| test (a trigger, a source) | probar |
| pause all / stop all | pausar todo / detener todos |
| more ▾ | más ▾ |
| digesting… / queued / failed | digiriendo… / en cola / falló |
| window (ticks) | ventana (ticks) |
| turn (rig) | giro |
| composite | compósito |
| venue | venue |
| loop (beat loop button) | loop |
| count (8 beats) | cuenta |
| binding / bound | vínculo / vinculado |
| colour / palette | color / paleta |
| speed / move (effects) | velocidad / movimiento |
