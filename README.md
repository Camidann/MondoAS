# Mondo

Mondo es mi asistente de escritorio. Puede conversar por texto o voz, responder con Ollama y mostrar una cara que cambia de expresión.

## Para empezar en Windows

Para usar Mondo, necesitas tener Ollama instalado y abierto. También tienes que descargar el modelo que aparece en `config.txt`. Por ejemplo:

```powershell
ollama pull llama3.2:1b
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
- **Hablar:** mantén presionado el atajo de voz mientras hablas y suéltalo al terminar. También puedes usar el botón **Hablar**.
- **Pantalla completa:** el botón muestra solo la cara. Presiona `F11` o `Esc` para volver.
- **Cambiar la cara:** usa **Elegir caras** para seleccionar imágenes. Mondo las ordena por nombre de archivo. También puedes estirar la cara con el mouse; el tamaño se guarda en `config.txt`.
- **Ocultar la cara:** usa `Alt+Shift+P` o el atajo que hayas configurado.
- **Abrir una aplicación por voz:** di, por ejemplo, «abrí Chrome» o «abrí Calculadora». Para cerrarla, di «cerrá Chrome». Si hay más de una coincidencia, Mondo te pide que aclares cuál.

Mientras hablas, Mondo muestra fragmentos de lo que reconoce. Cuando terminas, envía el texto a Ollama y muestra la respuesta en el chat. Si está activada la voz, también la lee en voz alta.

La voz y el reconocimiento se procesan localmente. Ollama debe estar abierto; puede estar en esta computadora o en otra a la que tengas acceso por red.

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
| `whisper_language` | Idioma del reconocimiento de voz. Usa `auto` para detectarlo. |
| `whisper_prompt` | Palabras que ayudan a reconocer nombres y expresiones. |
| `whisper_threads` | Hilos de CPU para el reconocimiento, de 1 a 16. |
| `whisper_beam_size` | Esfuerzo de búsqueda, de 1 a 5. Un número mayor puede tardar más. |
| `whisper_server_bin` | Ruta al servidor de reconocimiento Whisper. |
| `whisper_model` | Ruta al modelo de reconocimiento Whisper. |
| `avatar_file` | Imagen que se usa como cara inicial o de respaldo. |
| `avatar_sequence` | Lista de imágenes separadas por `\|`. |
| `avatar_enabled` | Indica si la cara aparece al iniciar. |
| `avatar_size` y `avatar_height` | Ancho y alto de la cara. El ancho puede ser de 64 a 512; el alto, de 64 a 768. |
| `voice_name` | Nombre exacto de una voz instalada. En Windows puedes verlas con `Add-Type -AssemblyName System.Speech; (New-Object System.Speech.Synthesis.SpeechSynthesizer).GetInstalledVoices() \| ForEach-Object { $_.VoiceInfo.Name }`. Déjalo vacío para usar la voz predeterminada. Esto selecciona una voz del sistema; no crea ni clona voces. |
| `speech_enabled` | Activa o desactiva la respuesta hablada. |

Los atajos deben ser distintos. Puedes escribir combinaciones como `CTRL+F9` o `ALT+SHIFT+M`. Si el sistema no permite registrar un atajo global, usa el botón de la ventana.

Las imágenes de la carpeta `FACES` se usan automáticamente con la configuración predeterminada. Los nombres indican cuándo se muestran: `idle` para reposo, `capturing` al empezar a grabar, `listen` mientras escucha, `thinking` mientras espera la respuesta, `speaking` al hablar, `warmup` durante la transcripción y `error` cuando ocurre un problema. Las imágenes numeradas se reproducen en orden alfabético.

## Reconocimiento de voz y micrófono

La primera vez, el sistema puede pedir permiso para usar el micrófono. Si no entra audio, elige otro dispositivo en el menú **Micrófono** y revisa la barra de nivel.

En Windows, habilita el acceso al micrófono para aplicaciones de escritorio en **Configuración > Privacidad y seguridad > Micrófono**. En Linux, revisa los permisos de ALSA o PipeWire.

Mondo envía fragmentos de audio de hasta cuatro segundos a Whisper mientras escucha. Usa archivos temporales y no guarda una grabación permanente.

## Linux y Raspberry Pi

Mondo necesita una sesión gráfica de escritorio. En Raspberry Pi, recomiendo una Pi 5 con Raspberry Pi OS de 64 bits y 8 GB de memoria. Ollama puede ejecutarse en la misma computadora o en otra.

En Debian, Ubuntu o Raspberry Pi OS, instala las dependencias:

```bash
sudo apt update
sudo apt install build-essential cmake pkg-config libasound2-dev libx11-dev libxi-dev libxrandr-dev libxcursor-dev libxinerama-dev libxkbcommon-dev libwayland-dev libudev-dev libssl-dev espeak-ng xdg-desktop-portal xdg-desktop-portal-gtk
```

Para cerrar aplicaciones por voz en X11, instala también `wmctrl`. Esa función no está disponible en Wayland. Algunos escritorios Wayland tampoco permiten atajos globales; en ese caso, usa el botón **Hablar**.

Instala Rust y compila Mondo y Whisper:

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
