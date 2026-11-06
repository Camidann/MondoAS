# Mondo

Mondo es mi asistente de escritorio. Puede conversar por texto o voz, responder con Ollama y mostrar una cara que cambia de expresión.

## Para empezar en Windows

Para usar Mondo, necesitas tener Ollama instalado y abierto. También tienes que descargar el modelo que aparece en `config.txt`. Por ejemplo:

```powershell
ollama pull qwen2.5:3b
```

Si tienes el ZIP portable, descomprímelo completo en una carpeta y abre `Mondo.exe`. No necesitas instalar Rust ni compilar nada.

Al clonar el código fuente desde GitHub, incluye el submódulo de Whisper:

```powershell
git clone --recurse-submodules URL_DE_TU_REPOSITORIO
```

Los modelos de voz y los archivos compilados no se suben al repositorio. Para descargar el modelo Whisper en Windows, ejecuta desde la carpeta del proyecto:

```powershell
& .\src\whisper.cpp\models\download-ggml-model.cmd base-q5_1
New-Item -ItemType Directory -Force whisper
Copy-Item .\src\whisper.cpp\models\ggml-base-q5_1.bin .\whisper\
```

## Cómo lo uso

- **Chat:** presiona `Ctrl+F` para abrirlo y vuelve a presionar `Ctrl+F` o `Esc` para cerrarlo.
- **Hablar manualmente:** por defecto, mantén presionado el atajo de voz mientras hablas y suéltalo al terminar. También puedes usar el botón **Hablar**.
- **Escucha automática:** puedes reanudarla desde el chat. Mondo escucha por el micrófono y, al detectar dos segundos de silencio, envía la consulta a Whisper.
- **Pantalla completa:** el botón muestra solo la cara. Presiona `F11` o `Esc` para volver.
- **Cambiar la cara:** usa **Elegir caras** para seleccionar imágenes. Mondo las ordena por nombre de archivo. También puedes estirar la cara con el mouse; el tamaño se guarda en `config.txt`.
- **Ocultar la cara:** usa `Alt+Shift+P` o el atajo que hayas configurado.
- **Abrir una aplicación por voz:** di, por ejemplo, «abrí Chrome» o «abrí Calculadora». Para cerrarla, di «cerrá Chrome». Si hay más de una coincidencia, Mondo te pide que aclares cuál.

En escucha automática, Mondo detecta la voz, espera dos segundos de silencio y luego transcribe y envía la consulta a Ollama. En modo manual muestra fragmentos de lo que reconoce mientras hablas. Muestra la respuesta en el chat; la lectura en voz alta se puede activar con `speech_enabled=true`.

La voz y el reconocimiento se procesan localmente. Ollama debe estar abierto; puede estar en esta computadora o en otra a la que tengas acceso por red.

### Ollama en otra computadora de la red

Si Mondo corre en una Raspberry Pi y Ollama en una PC con Windows, configura Ollama en Windows para escuchar en la red local. En PowerShell ejecuta `setx OLLAMA_HOST "0.0.0.0:11434"` y reinicia Ollama desde el icono de la bandeja. Permite el puerto en el firewall solo para redes privadas. En el `config.txt` de la Raspberry, `ollama_url` debe usar la IPv4 de esa PC; este clon está configurado con `192.168.1.17`, que puedes cambiar si la dirección de la PC cambia. Reserva esa IP en el router para que no varíe. Desde la Pi, comprueba la conexión con `curl http://192.168.1.17:11434/api/tags`.

## Configuración

Mondo crea `config.txt` si no existe. Puedes cambiar las opciones con cualquier editor de texto. Las rutas relativas, como `avatar.png`, se buscan desde la carpeta donde está ese archivo. Al ejecutar Mondo desde `target\debug` o `target\release`, se usa el `config.txt` de la carpeta principal del proyecto.

Estas son las opciones principales:

| Opción | Para qué sirve |
|---|---|
| `assistant_name` | Nombre que aparece en la ventana. |
| `ollama_url` | Dirección de Ollama. Por defecto es `http://localhost:11434`. |
| `model` | Modelo de Ollama que quieres usar. |
| `system_prompt` | Instrucciones y personalidad del asistente. |
| `temperature` | Ajusta qué tan variadas son las respuestas. |
| `context_size` | Cantidad de contexto que se envía al modelo. |
| `input_device` | Micrófono que se usa. Déjalo vacío para elegir el predeterminado. |
| `push_to_talk_hotkey` | Atajo para empezar y terminar de hablar. |
| `avatar_toggle_hotkey` | Atajo para mostrar u ocultar la cara. |
| `whisper_language` | Idioma del reconocimiento de voz. Para hablar en español, usa `es` en lugar de `auto` para evitar que Whisper confunda el idioma. |
| `whisper_prompt` | Palabras que ayudan a reconocer nombres y expresiones. |
| `whisper_threads` | Hilos de CPU para el reconocimiento, de 1 a 16. |
| `whisper_beam_size` | Esfuerzo de búsqueda, de 1 a 5. Un número mayor puede tardar más. |
| `whisper_server_bin` | Ruta al servidor de reconocimiento Whisper. |
| `whisper_model` | Ruta al modelo de reconocimiento Whisper. |
| `avatar_file` | Imagen que se usa como cara inicial o de respaldo. |
| `avatar_sequence` | Lista de imágenes separadas por `\|`. |
| `avatar_enabled` | Indica si la cara aparece al iniciar. |
| `avatar_size` y `avatar_height` | Ancho y alto de la cara. El ancho puede ser de 64 a 512; el alto, de 64 a 768. |
| `voice_name` | Voz del sistema en Windows/macOS. En Linux, déjalo vacío para usar Piper; un nombre configurado selecciona una voz de eSpeak. En Windows puedes ver voces instaladas con `Add-Type -AssemblyName System.Speech; (New-Object System.Speech.Synthesis.SpeechSynthesizer).GetInstalledVoices() \| ForEach-Object { $_.VoiceInfo.Name }`. |
| `piper_model` | Ruta al modelo de voz Piper en Linux. El archivo `.onnx.json` correspondiente debe estar junto al modelo. |
| `speech_enabled` | Activa o desactiva la respuesta hablada; `false` deja las respuestas solo en texto. |

Los atajos deben ser distintos. Puedes escribir combinaciones como `CTRL+F9` o `ALT+SHIFT+M`. Si el sistema no permite registrar un atajo global, usa el botón de la ventana.

Las imágenes de la carpeta `FACES` se usan automáticamente con la configuración predeterminada. Los nombres indican cuándo se muestran: `idle` para reposo, `sleep` o `dorm` para dormir tras 20 segundos de inactividad, `capturing` al empezar a grabar, `listen` mientras escucha, `thinking` mientras espera la respuesta, `speaking` al hablar, `warmup` durante la transcripción y `error` cuando ocurre un problema. Las imágenes numeradas se reproducen en orden alfabético.

## Reconocimiento de voz y micrófono

La primera vez, el sistema puede pedir permiso para usar el micrófono. La escucha automática se activa al iniciar Mondo y mantiene abierto el micrófono; usa **Pausar escucha automática** cuando no quieras que escuche. Si no entra audio o no detecta tu voz, elige otro dispositivo en el menú **Micrófono** y revisa la barra de nivel.

En Windows, habilita el acceso al micrófono para aplicaciones de escritorio en **Configuración > Privacidad y seguridad > Micrófono**. En Linux, revisa los permisos de ALSA o PipeWire.

En escucha automática, Mondo conserva 500 ms de audio reciente en memoria para no cortar el comienzo de las palabras. Selecciona el canal del micrófono con mejor nivel, convierte el audio a 16 kHz con un filtro antialias y ajusta la ganancia de la voz sin permitir saturación. Al detectar dos segundos de silencio, transcribe el turno completo con Whisper. Los WAV temporales se eliminan al terminar; no se guarda una grabación permanente. Si Whisper reconoce otro idioma, revisa que `whisper_language=es` en `config.txt`.

## Linux y Raspberry Pi

Mondo necesita una sesión gráfica de escritorio. También puede compilarse para Raspberry Pi 3B (ARM de 32 o 64 bits); en ese modelo, por sus 1 GB de memoria, conviene ejecutar Ollama en otra computadora accesible por red. Una Pi 5 con Raspberry Pi OS de 64 bits ofrece más velocidad.

En Debian, Ubuntu o Raspberry Pi OS, instala las dependencias:

```bash
sudo apt update
sudo apt install build-essential cmake pkg-config libasound2-dev alsa-utils libgl1-mesa-dev libx11-dev libxi-dev libxrandr-dev libxcursor-dev libxinerama-dev libxkbcommon-dev libxkbcommon-x11-0 libwayland-dev libudev-dev espeak-ng xdg-desktop-portal xdg-desktop-portal-gtk
```

Para cerrar aplicaciones por voz en X11, instala también `wmctrl`. Esa función no está disponible en Wayland. Algunos escritorios Wayland tampoco permiten atajos globales; en ese caso, usa el botón **Hablar**.

Para una voz hablada más natural que eSpeak, instala Piper compatible con la arquitectura de la Raspberry y deja el ejecutable `piper` en `PATH`. En Raspberry Pi OS de 64 bits, puedes instalarlo con `pipx` para mantener sus dependencias aisladas:

```bash
sudo apt install pipx
pipx install piper-tts
pipx ensurepath
```

En Raspberry Pi OS de 32 bits, usa el paquete `armv7` de la [guía oficial](https://github.com/OHF-Voice/piper1-gpl/blob/main/docs/CLI.md#binary-releases). Si lo instalaste con `pipx`, Mondo también busca el ejecutable en `~/.local/bin/piper`, incluso si el escritorio no hereda esa ruta. Después, descarga la voz `es_MX-ald-medium` junto a su archivo de configuración, en la carpeta de Mondo:

```bash
curl -L https://huggingface.co/rhasspy/piper-voices/resolve/main/es/es_MX/ald/medium/es_MX-ald-medium.onnx -o es_MX-ald-medium.onnx
curl -L https://huggingface.co/rhasspy/piper-voices/resolve/main/es/es_MX/ald/medium/es_MX-ald-medium.onnx.json -o es_MX-ald-medium.onnx.json
```

En `config.txt`, deja `voice_name=` vacío, configura `piper_model=es_MX-ald-medium.onnx` y activa `speech_enabled=true`. Tanto el modelo como su archivo `.onnx.json` deben estar junto a `config.txt` (o ajusta la ruta relativa). Esta voz es de español mexicano; si prefieres otra variante, cambia el modelo y su `.json`. Para desactivar la lectura en voz alta, vuelve a poner `speech_enabled=false`. Piper y la voz se ejecutan localmente; no se envía el texto a un servicio de voz. Consulta también la [guía oficial de instalación de Piper](https://github.com/OHF-Voice/piper1-gpl).

Instala Rust estable y compila Mondo y Whisper:
q
```bash
cargo build --release
cmake -S src/whisper.cpp -B src/whisper.cpp/build -DCMAKE_BUILD_TYPE=Release -DWHISPER_BUILD_SERVER=ON
cmake --build src/whisper.cpp/build -j2
bash src/whisper.cpp/models/download-ggml-model.sh base-q5_1
mkdir -p whisper
cp src/whisper.cpp/build/bin/whisper-server whisper/
cp src/whisper.cpp/models/ggml-base-q5_1.bin whisper/
```

Después, ejecuta `target/release/Mondo` desde el escritorio. Si la computadora tiene poca memoria o es lenta, puedes usar un modelo Whisper más pequeño y cambiar `whisper_model` en `config.txt`.

### Ajustes para Raspberry Pi 3B

En una Pi 3B usa el modelo compacto `tiny-q5_1` para reconocimiento y deja Ollama en otra computadora. Para reducir la memoria necesaria durante la compilación, ejecuta desde la carpeta del proyecto:

```bash
CARGO_BUILD_JOBS=1 cargo build --release
cmake -S src/whisper.cpp -B src/whisper.cpp/build -DCMAKE_BUILD_TYPE=Release -DWHISPER_BUILD_SERVER=ON -DWHISPER_BUILD_EXAMPLES=ON -DWHISPER_BUILD_TESTS=OFF -DGGML_NATIVE=ON
cmake --build src/whisper.cpp/build --target whisper-server --parallel 1
bash src/whisper.cpp/models/download-ggml-model.sh tiny-q5_1
mkdir -p whisper
cp src/whisper.cpp/build/bin/whisper-server whisper/
cp src/whisper.cpp/models/ggml-tiny-q5_1.bin whisper/
```

En instalaciones nuevas de ARM, la configuración inicial usa 2 hilos, búsqueda mínima y el modelo `tiny-q5_1`. Si ya existe `config.txt`, cambia `whisper_threads=2`, `whisper_beam_size=1` y `whisper_model=whisper/ggml-tiny-q5_1.bin`. Para compilar cruzado, usa `armv7-unknown-linux-gnueabihf` en Raspberry Pi OS de 32 bits o `aarch64-unknown-linux-gnu` en 64 bits, junto con el toolchain y sysroot correspondientes; compila Whisper de forma nativa en la Pi.

## Crear el ZIP portable de Windows

Para generar el ZIP necesitas Rust, CMake y Visual Studio Build Tools con las herramientas de C++. Desde PowerShell, ejecuta:

```powershell
cargo build --release
cmake -S .\src\whisper.cpp -B .\src\whisper.cpp\build
cmake --build .\src\whisper.cpp\build --config Release
& .\src\whisper.cpp\models\download-ggml-model.cmd base-q5_1
.\package-windows.ps1
```

El ZIP queda en `dist\Mondo-portable.zip`. Incluye Mondo, Whisper, el modelo de voz, las caras y la configuración.

## Desarrollo

Desde la carpeta del proyecto, ejecuta las pruebas y compila así:

```powershell
cargo test
cargo build --release
```

En Linux puedes ejecutar los mismos comandos si instalaste las dependencias indicadas arriba.
