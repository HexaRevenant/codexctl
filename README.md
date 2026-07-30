<p align="center">
  <img src="mascota.png" alt="CodexCTL" width="320">
</p>

<h1 align="center">⌘ CodexCTL</h1>

<p align="center"><strong>Multi-account quota tracker for OpenAI Codex — Rust + Tauri</strong></p>

## ¿Para qué sirve?

Si tenés **varias cuentas de OpenAI Codex** (Plus, Team, Free) y querés:
- Ver cuánta quota te queda en **cada una** sin tener que switchear manualmente
- Saber cuál tiene **disponible** para seguir trabajando
- Cambiar de cuenta **al instante** con un solo comando
- No perder tiempo logueándote y deslogueándote cada vez

CodexCTL te muestra todo en una tabla: el porcentaje usado, el disponible, y cuándo se reinicia cada ventana (5h, 7d). Todo local, todo directo desde la API de OpenAI.

```
ID       Cuenta       Email                    Plan     5h%  Disp.  7d%  Disp.  Reinicio
team-1   Cuenta 1     ignacio@innobyte.cl      team     —    —      90%  10%    mié 05 ago 14:29
free-1   Cuenta 4     ignacio@innobyte.cl      free     100% 0%     —    —      jue 27 ago 19:22
```

## Características

```
ID       Cuenta       Email                    Plan     5h%  Disp.  7d%  Disp.  Reinicio
team-1   Cuenta 1     ignacio@innobyte.cl      team     —    —      90%  10%    mié 05 ago 14:29
free-1   Cuenta 4     ignacio@innobyte.cl      free     100% 0%     —    —      jue 27 ago 19:22
```

## Características

- **Quota en vivo** — consulta directa a la API de OpenAI, no estimaciones
- **Múltiples cuentas** — agrega todas las que quieras (Plus, Team, Free)
- **Switch instantáneo** — cambia la cuenta activa de Codex CLI al instante
- **Refresh automático** — la app de escritorio se actualiza cada 5 minutos
- **Multipataforma** — Linux (.deb, .AppImage), macOS (.dmg), Windows (.msi)
- **Hecho en Rust** — binario único, sin dependencias de runtime

## Instalación

### Linux

```bash
# Descargar AppImage (recomendado — portable, no necesita nada)
chmod +x codexctl_0.1.0_amd64.AppImage
./codexctl_0.1.0_amd64.AppImage

# O instalar .deb
sudo dpkg -i codexctl_0.1.0_amd64.deb

# CLI
sudo cp codexctl /usr/local/bin/
```

### macOS / Windows

Descargar desde [Releases](https://github.com/HexaRevenant/codexctl/releases).

## Uso

### CLI

```bash
codexctl list              # Ver todas las cuentas con quota
codexctl add "Mi cuenta"   # Agregar nueva cuenta (abre el browser)
codexctl switch <id>       # Cambiar cuenta activa
codexctl rename <id> "nuevo nombre"  # Renombrar
codexctl remove <id>       # Eliminar cuenta
codexctl refresh           # Forzar refresh de tokens
codexctl status            # Ver cuenta activa
```

### App de escritorio

```bash
codexctl-tauri             # Abre la ventana gráfica
```

O desde el menú de aplicaciones → **CodexCTL**.

## Cómo funciona

1. Cada cuenta se agrega ejecutando `codex login` con un `CODEX_HOME` aislado
2. El `auth.json` de cada cuenta se guarda en `~/.local/share/codexctl/homes/<uuid>/`
3. Para ver quota, se llama a la API oficial de OpenAI:

```
GET https://chatgpt.com/backend-api/wham/usage
Authorization: Bearer <access_token>
ChatGPT-Account-Id: <account_id>
```

4. **`used_percent`** → columna 5h% / 7d%
5. **`100 - used_percent`** → columna Disp. (disponible)
6. Al cambiar de cuenta, se copia el `auth.json` target a `~/.codex/`

## Build desde fuente

```bash
git clone git@github.com:HexaRevenant/codexctl.git
cd codexctl

# CLI
cargo build --release
./target/release/codexctl list

# App de escritorio (requiere GTK, WebKit, etc.)
cd src-tauri
PKG_CONFIG_PATH=/usr/lib/x86_64-linux-gnu/pkgconfig cargo tauri build
```

## Licencia

MIT
