# buscatxt

Aplicacion de consola escrita en Rust que busca un texto, con comodines `*` y `?`, dentro de archivos de una o varias extensiones, recorriendo las carpetas de forma recursiva. Busca en archivos de texto y tambien dentro de documentos de Word, Excel, PowerPoint (actuales y 97-2003), LibreOffice y PDF. Funciona en Linux y Windows 11.

No usa dependencias externas: solo la biblioteca estandar de Rust (incluidos los lectores de ZIP, XML, PDF y del formato binario de Office). Version actual: **0.4.0** (requiere Rust 1.88 o superior para compilar).

## Descargar

Los ejecutables listos para usar estan en la pagina de **[Releases](https://github.com/infodao-sys/buscatxt/releases/latest)** (no hace falta instalar Rust ni compilar):

| Sistema | Paquete con instalador | Solo el ejecutable |
|---|---|---|
| Windows 10/11 (64 bits) | [buscatxt-0.4.0-windows-x86_64.zip](https://github.com/infodao-sys/buscatxt/releases/download/v0.4.0/buscatxt-0.4.0-windows-x86_64.zip) | [buscatxt-windows-x86_64.exe](https://github.com/infodao-sys/buscatxt/releases/latest/download/buscatxt-windows-x86_64.exe) |
| Linux (x86_64) | [buscatxt-0.4.0-linux-x86_64.tar.gz](https://github.com/infodao-sys/buscatxt/releases/download/v0.4.0/buscatxt-0.4.0-linux-x86_64.tar.gz) | [buscatxt-linux-x86_64](https://github.com/infodao-sys/buscatxt/releases/latest/download/buscatxt-linux-x86_64) |

- **Windows**: descomprime el ZIP y haz doble clic en `scripts\instalar-windows.bat`. Como el programa no esta firmado digitalmente, Windows puede mostrar "Windows protegio su PC": elige "Mas informacion" > "Ejecutar de todas formas".
- **Linux**: descomprime y ejecuta el instalador:

  ```bash
  tar -xzf buscatxt-0.4.0-linux-x86_64.tar.gz
  cd buscatxt-0.4.0-linux-x86_64
  ./scripts/instalar-linux.sh
  ```

- Cada paquete incluye un archivo `INSTALAR.txt` con estos pasos.
- Para comprobar que la descarga no fue alterada, compara su huella con el archivo `SHA256SUMS.txt` de la Release (`sha256sum -c SHA256SUMS.txt --ignore-missing` en Linux, `Get-FileHash archivo` en PowerShell).

Los mismos ejecutables tambien estan en la carpeta `dist/` del repositorio.

## Instalar el comando

Los scripts de `scripts/` instalan `buscatxt` como comando, para usarlo desde cualquier carpeta. No descargan nada: copian el ejecutable ya incluido en `dist/` (o el compilado con `cargo build --release` si no existe) y no necesitan permisos de administrador.

### Linux

```bash
git clone https://github.com/infodao-sys/buscatxt.git
cd buscatxt
./scripts/instalar-linux.sh
```

- Copia el comando a `~/.local/bin` (solo para tu usuario). Si esa carpeta no esta en el `PATH`, agrega una linea a `~/.bashrc` (y a `~/.zshrc` si existe), marcada con `# agregado por buscatxt`. Abre una terminal nueva para usarlo.
- `./scripts/instalar-linux.sh --sistema`: lo instala para todos los usuarios en `/usr/local/bin` (pide `sudo`).
- `./scripts/instalar-linux.sh --desinstalar`: quita el comando de ambos lugares y la linea agregada al `PATH`.
- En equipos que no son x86_64 (por ejemplo ARM o Raspberry Pi) compila primero con `cargo build --release`; el script usa ese ejecutable.

### Windows

Descarga el proyecto (boton **Code > Download ZIP** en GitHub, o `git clone`), descomprimelo y haz **doble clic** en `scripts\instalar-windows.bat`. O desde PowerShell, en la carpeta del proyecto:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\instalar-windows.ps1
```

- Copia el comando a `%LOCALAPPDATA%\Programs\buscatxt` y agrega esa carpeta al `PATH` de tu usuario (sin permisos de administrador). Abre una ventana nueva de PowerShell o cmd para usarlo.
- Para desinstalar: doble clic en `scripts\desinstalar-windows.bat` (o el comando anterior con `-Desinstalar`). Quita la carpeta y la entrada del `PATH`, sin tocar las demas.
- `-ExecutionPolicy Bypass` permite ejecutar el script descargado solo esta vez, sin cambiar la configuracion de seguridad de Windows.

Despues de instalar, en cualquier terminal:

```text
buscatxt --version
buscatxt "texto a buscar" C:\ruta\o\carpeta -d
```

## Compilar y ejecutar en Linux

```bash
cargo build --release
./target/release/buscatxt "texto a buscar" /ruta/donde/buscar
```

El ejecutable queda en `target/release/buscatxt`. Para tenerlo siempre disponible como comando, usa `./scripts/instalar-linux.sh` (ver [Instalar el comando](#instalar-el-comando)).

## Compilar y ejecutar en Windows 11

1. Instala Rust desde <https://rustup.rs> (descarga `rustup-init.exe` y ejecutalo). El instalador ofrece instalar las "Visual Studio Build Tools", que son necesarias para compilar con el target por defecto de Windows (MSVC).
2. Abre PowerShell en la carpeta del proyecto y ejecuta:

```powershell
cargo build --release
.\target\release\buscatxt.exe "texto a buscar" C:\ruta\donde\buscar
```

El ejecutable queda en `target\release\buscatxt.exe`. Para tenerlo siempre disponible como comando, usa `scripts\instalar-windows.bat` (ver [Instalar el comando](#instalar-el-comando)); sin compilar nada, ese script instala el binario ya incluido en `dist\`.

## Uso en Windows

### Comillas segun la consola

| Consola | Como escribir el texto | Detalle |
|---|---|---|
| cmd | `buscatxt "error*disco" C:\logs` | Los comodines llegan tal cual al programa. |
| PowerShell | `buscatxt 'error*disco' C:\logs` | Usa **comillas simples**: dentro de comillas dobles PowerShell reemplaza `$variables` antes de llamar al programa. Por ejemplo, `"precio $total"` llega como `precio ` (con `$total` vacio), y `"$total"` llega vacio. Con `'precio $total'` llega tal cual. Si necesitas comillas dobles, escapa el dolar: `"precio `$total"`. |
| Archivo `.bat` | `buscatxt "100%% listo" C:\docs` | En un `.bat` el signo `%` se escribe doble. |

En Windows, ni cmd ni PowerShell reemplazan `*` o `?` por nombres de archivo, asi que los comodines llegan al programa aunque no lleven comillas. Igual hay que usar comillas si el texto tiene espacios: `factura 2025` sin comillas llega como dos argumentos y el programa muestra un error pidiendo comillas.

> PowerShell instalado en Linux o macOS (`pwsh`) **si** reemplaza los comodines sin comillas por nombres de archivo, igual que bash. Usa comillas simples tambien ahi.

### Ejemplos

```bat
:: cmd
buscatxt "factura ?2025" C:\Users\%USERNAME%\Documents -e txt,csv
buscatxt "TODO" "C:\Mis proyectos" -e rs,md -l
buscatxt "C:\temp" D:\scripts -e bat,ps1
```

```powershell
# PowerShell
buscatxt 'error*disco' C:\logs -i
buscatxt 'TODO' $HOME\proyectos -e rs -e md -l
```

- Las rutas pueden usar `\` o `/`, letra de unidad (`D:\datos`) o rutas de red (`\\servidor\carpeta`).
- Las rutas con espacios van entre comillas.
- Los resultados muestran siempre la ruta completa con `\`, por ejemplo `C:\logs\2025\app.txt:12:error en disco`.

### Combinar con otros comandos

```bat
buscatxt "error" C:\logs -l > archivos_con_error.txt
buscatxt "error" C:\logs | findstr /i "disco"
buscatxt "error" C:\logs | more
```

```powershell
buscatxt 'error' C:\logs | Select-String 'disco'
buscatxt 'error' C:\logs | Select-Object -First 5
```

Cortar la salida antes de tiempo (`| more`, `Select-Object -First`, `| head`) es seguro: el programa detecta que la tuberia se cerro y termina sin mostrar errores.

### Codigo de salida en Windows

```bat
buscatxt "error" C:\logs >NUL
if %errorlevel%==0 echo Se encontro el texto
if %errorlevel%==1 echo No se encontro
if %errorlevel%==2 echo Hubo un error
```

En PowerShell el codigo queda en `$LASTEXITCODE`:

```powershell
buscatxt 'error' C:\logs > $null
if ($LASTEXITCODE -eq 0) { 'Se encontro el texto' }
```

### Particularidades de Windows

- **Archivos UTF-16**: los que guarda el Bloc de notas como "Unicode" y los que crea PowerShell 5 con `>` o `Out-File` se leen correctamente (se detectan por su BOM).
- **Archivos ANSI / Windows-1252**: los archivos antiguos con acentos (`acción`, `año`, `€`) se leen correctamente aunque no sean UTF-8.
- **Saltos de linea `\r\n`**: se manejan igual que `\n`; el `\r` no aparece en los resultados.
- **Uniones (junctions) y enlaces simbolicos**: no se siguen, igual que en Linux. Asi se evita, por ejemplo, entrar en bucles con carpetas como `C:\Users\<usuario>\Application Data`.
- **Mensajes del sistema**: los errores de acceso los genera Windows y salen en el idioma del sistema, por ejemplo `aviso: no se pudo leer C:\...: Acceso denegado. (os error 5)`. Son avisos: la busqueda sigue con los demas archivos.
- **Colores**: se ven en Windows Terminal (la consola por defecto de Windows 11). Si en alguna consola antigua aparecen codigos como `←[35m`, define la variable `NO_COLOR` (`set NO_COLOR=1` en cmd o `$env:NO_COLOR=1` en PowerShell).
- **Rutas largas**: se admiten rutas de mas de 260 caracteres.

## Uso

```text
buscatxt <TEXTO> [RUTA] [OPCIONES]

ARGUMENTOS:
    <TEXTO>    Texto a buscar (admite comodines)
    [RUTA]     Archivo o carpeta donde buscar (por defecto: .)

OPCIONES:
    -i, --ignore-case          Ignora mayusculas y minusculas
    -l, --files-with-matches   Muestra solo la ruta de los archivos
    -e, --extension <EXT>      Extensiones a buscar, separadas por comas o
                               repitiendo la opcion (por defecto: txt)
    -d, --documentos           Busca tambien en documentos de Office,
                               LibreOffice y PDF
    -a, --all                  Busca en todos los archivos
    -F, --fixed-strings        Trata * y ? como texto normal (sin comodines)
    -j, --threads <N>          Cantidad de hilos de busqueda
    -h, --help                 Muestra la ayuda
    -V, --version              Muestra la version
```

Las opciones cortas se pueden agrupar (`-il`) y van antes o despues de los argumentos.

## Documentos: Word, Excel, PowerPoint, LibreOffice y PDF

Con `-d` se busca, ademas de en los `.txt`, dentro del texto de estos documentos:

| Programa | Extensiones | Donde se busca | Ubicacion que se muestra |
|---|---|---|---|
| Word | `docx`, `docm`, `dotx`, `dotm` | texto, tablas, encabezados, pies, notas al pie y finales, comentarios | `parrafo 12`, `encabezado`, `nota al pie`, `comentario` |
| Excel | `xlsx`, `xlsm`, `xltx`, `xltm` | todas las hojas; textos, numeros y fechas de las celdas | `hoja 'Ventas', fila 7` |
| PowerPoint | `pptx`, `pptm`, `potx`, `ppsx` | texto de las diapositivas (en el orden real) y notas del orador | `diapositiva 3`, `notas diapositiva 3` |
| Word 97-2003 | `doc`, `dot` | texto, tablas, encabezados y pies, notas, comentarios, cuadros de texto | `parrafo 12`, `encabezado o pie`, `nota al pie`, `comentario` |
| Excel 97-2003 | `xls`, `xlt` | todas las hojas; textos, numeros, fechas y resultados de formulas | `hoja 'Ventas', fila 7` |
| PowerPoint 97-2003 | `ppt`, `pot`, `pps` | diapositivas y notas del orador | `diapositiva 3`, `notas diapositiva 3` |
| LibreOffice Writer | `odt`, `ott` | texto, tablas, encabezados, pies, notas y comentarios | `parrafo 12`, `encabezado`... |
| LibreOffice Calc | `ods`, `ots` | todas las hojas | `hoja 'Ventas', fila 7` |
| LibreOffice Impress | `odp`, `otp` | diapositivas y notas | `diapositiva 3` |
| PDF | `pdf` | texto de todas las paginas | `pagina 5` |

```bash
buscatxt "contrato*2025" ~/Documentos -d -i      # .txt y todos los documentos
buscatxt "Pérez" . -e docx,pdf                    # solo Word y PDF
buscatxt "factura" . -e xlsx,xls,ods -l           # solo hojas de calculo, solo rutas
```

Ejemplo de salida redirigida:

```text
/home/ana/informes/anual.docx:parrafo 2:El contrato número 2025-117 fue firmado por José Pérez
/home/ana/informes/anual.pdf:pagina 1:El contrato número 2025-117 fue firmado por José Pérez
/home/ana/ventas.xlsx:hoja 'Ventas', fila 2:José Pérez | Málaga | 1500.5 | 2025-03-01
```

- En las hojas de calculo cada fila se muestra con sus celdas separadas por ` | `. Las fechas de Excel se muestran como `aaaa-mm-dd` (y las horas como `hh:mm:ss`), asi que se pueden buscar: `buscatxt "2025-03-*" . -e xlsx`.
- Con `-a` tambien se leen los documentos (se reconocen por la extension).
- Los comodines, `-i`, `-l` y el resto de opciones funcionan igual que con los archivos de texto.

**Lo que no se puede leer** (se muestra un aviso y la busqueda continua):

- Documentos protegidos con contrasena (Word, Excel, PowerPoint, LibreOffice y PDF cifrados).
- PDF escaneados: el texto es una imagen; haria falta reconocimiento de texto (OCR).
- PDF cuyas fuentes no indican a que letra corresponde cada codigo (poco habitual; tampoco se pueden copiar y pegar desde un visor de PDF).
- Versiones de Office anteriores a 97: Word 6/95 y Excel 5/95 (se informa con un aviso). Se pueden convertir con Word o LibreOffice.

En los PDF los saltos de linea y los espacios entre palabras se deducen de la posicion del texto en la pagina, igual que al copiar texto desde un visor; en documentos con varias columnas el orden puede no ser el de lectura.

## Comodines

| Comodin | Significado | Ejemplo | Encuentra |
|---|---|---|---|
| `*` | cualquier cantidad de caracteres, incluso ninguno (dentro de la linea) | `error*disco` | `error en disco`, `errordisco` |
| `?` | exactamente un caracter | `a?o` | `ano`, `año`, `a1o` |
| `\*` | un asterisco literal | `10\*2` | `10*2` |
| `\?` | un signo de interrogacion literal | `hola\?` | `hola?` |
| `\\` | una barra invertida literal | `C:\\temp` | `C:\temp` |

- El texto se busca en cualquier parte de la linea: no hace falta poner `*` al inicio ni al final.
- Una barra que no va antes de `*`, `?` o `\` se toma literal, asi que `"C:\temp"` tambien funciona.
- Con `-F` los comodines se desactivan.
- **Escribe el texto entre comillas** si lleva espacios, `*` o `?`. En Linux (bash y tambien PowerShell `pwsh`), sin comillas, la terminal reemplaza el comodin por nombres de archivo (`hol*` pasa a ser `hola1.txt hola2.txt`). Si eso ocurre y llegan mas de dos argumentos, el programa muestra un error en vez de buscar otra cosa. Si el comodin coincide con un solo archivo, la terminal lo reemplaza sin que el programa pueda notarlo, por eso las comillas son importantes. En Windows (cmd y PowerShell) no ocurre, pero las comillas tampoco molestan. Ver tambien [Uso en Windows](#uso-en-windows).
- Los comodines van en el texto, no en la ruta: para elegir tipos de archivo usa `-e` (`-e txt,log`), no `C:\docs\*.txt`.

## Ejemplos

```bash
buscatxt "hola mundo"
buscatxt "error*disco" ./registros -i
buscatxt "factura ?2025" . -e txt,csv,log
buscatxt "TODO" C:\proyectos -e rs -e md -l
buscatxt "precio: 10\*2" . -a
```

## Salida

En la terminal, cada archivo con coincidencias muestra su nombre y su carpeta, y debajo cada linea encontrada con su numero y la coincidencia resaltada. Al final se muestra un resumen:

```text
Archivo: informe.txt
Carpeta: /home/usuario/documentos/2025
  linea 12: error en disco duro
  linea 40: ERROR: Disco lleno

2 lineas encontradas en 1 archivo.
```

Cuando la salida se redirige a un archivo o a otro programa (`> resultado.txt`, `| findstr`, `| grep`), se usa una linea por coincidencia con la ruta completa: `ruta_completa:numero_de_linea:contenido`. Con `-l` solo se muestra la ruta completa de cada archivo. La variable de entorno `NO_COLOR` desactiva los colores.

Los resultados siempre salen en orden alfabetico de carpetas y archivos, aunque la busqueda se haga en paralelo.

## Codificaciones de texto

- UTF-8, con o sin BOM.
- UTF-16 con BOM (Bloc de notas "Unicode", redirecciones de PowerShell 5).
- Windows-1252 / Latin-1 (archivos antiguos de Windows con acentos): se detecta automaticamente en las lineas que no son UTF-8 valido.
- Archivos binarios (con bytes nulos al inicio): solo se informa `[archivo binario: contiene el texto]`, sin mostrar su contenido. Los documentos de Office, LibreOffice y PDF no se tratan como binarios: se extrae su texto (ver [Documentos](#documentos-word-excel-powerpoint-libreoffice-y-pdf)).

## Seguridad y limites

Los documentos (ZIP, XML, PDF) son formatos complejos y un archivo puede estar preparado para atacar al programa que lo lee. Defensas incluidas:

- **Sin codigo de terceros**: el lector ZIP, el descompresor DEFLATE, el lector XML, el de PDF y el del contenedor OLE de Office 97-2003 estan escritos en Rust seguro, sin `unsafe` y sin bibliotecas externas.
- **Bombas de descompresion**: un documento pequeno que se expande a gigabytes se corta al llegar a 128 MiB por parte y 512 MiB por documento, aunque el archivo mienta sobre su tamano.
- **XML**: no se procesan DTD ni entidades definidas por el documento, asi que no hay ataques "billion laughs" (expansion exponencial) ni XXE (lectura de archivos del disco o de la red).
- **PDF**: limites de profundidad de anidamiento, de cantidad de paginas y de tamano de cada flujo; los ciclos en el arbol de paginas y las referencias circulares se detectan.
- **Office 97-2003**: las cadenas de sectores del contenedor OLE no pueden tener ciclos, los tamanos y posiciones se validan, y los recorridos de registros tienen un tope (un archivo con contenedores anidados a proposito no puede hacer el recorrido interminable).
- **Fallos inesperados**: si aun asi la lectura de un documento fallara, el error se captura y se informa como aviso de ese archivo, sin detener la busqueda.
- Se probo con mas de 5300 documentos danados a proposito (bytes alterados, truncados, XML, flujos PDF y estructuras OLE corruptas) sin ningun cuelgue ni fallo.

Para todos los archivos:

- El programa solo lee archivos: no ejecuta comandos, no carga librerias, no abre conexiones de red y no contiene codigo `unsafe`.
- Los caracteres de control (por ejemplo `ESC`) y los de direccion de texto (bidi) presentes en el contenido o en los nombres de archivo se muestran como `\x{1B}`, para que un archivo no pueda inyectar secuencias de escape en la terminal ni disfrazar su nombre.
- No se siguen enlaces simbolicos ni uniones de Windows (evita bucles y salir de la carpeta indicada).
- Los archivos se leen linea por linea, sin cargarlos completos en memoria.
- Las lineas de mas de 1 MiB se procesan por bloques: la memoria usada se mantiene acotada y la salida muestra `[linea de mas de 1 MiB, recortada]` con unos 100 caracteres de contexto a cada lado de la coincidencia. Los comodines funcionan tambien a traves de los bloques.
- Si la salida o los avisos no se pueden escribir porque el programa que los recibe ya termino (`| head`, `| more`), el programa termina sin errores, tanto en Linux como en Windows. En Windows esto llega con los codigos de sistema 109, 232 o 233, que tambien se tratan como cierre normal.

## Codigos de salida

- `0`: se encontro al menos una coincidencia.
- `1`: no se encontro ninguna coincidencia.
- `2`: error de uso o de acceso a la ruta.

## Estructura del codigo

| Archivo | Contenido |
|---|---|
| `src/main.rs` | argumentos, comodines, lectura de archivos de texto, recorrido de carpetas en paralelo y salida |
| `src/docs/mod.rs` | tipos de documento, limites de seguridad y captura de fallos |
| `src/docs/inflate.rs` | descompresor DEFLATE/zlib |
| `src/docs/zip.rs` | lector de archivos ZIP |
| `src/docs/xml.rs` | lector XML minimo (sin DTD ni entidades externas) |
| `src/docs/office.rs` | texto de DOCX, XLSX, PPTX, ODT, ODS y ODP |
| `src/docs/cfb.rs` | lector del contenedor OLE (Compound File) de Office 97-2003 |
| `src/docs/legacy.rs` | texto de DOC, XLS y PPT |
| `scripts/instalar-linux.sh` | instala o desinstala el comando en Linux |
| `scripts/instalar-windows.ps1` (+ `.bat`) | instala o desinstala el comando en Windows |
| `scripts/build-windows.sh` | compila el `.exe` de Windows desde Linux |
| `src/docs/pdf.rs` | texto de PDF: objetos, paginas, fuentes y posicion del texto |

## Compilacion cruzada desde Linux hacia Windows

El target de Rust y MinGW-w64 ya estan instalados localmente (sin root) en `~/.local/mingw64`. Para recompilar el `.exe`:

```bash
./scripts/build-windows.sh
```

El resultado queda en `target/x86_64-pc-windows-gnu/release/buscatxt.exe`.

Si se instala desde cero en otro equipo:

```bash
rustup target add x86_64-pc-windows-gnu
sudo apt install mingw-w64
./scripts/build-windows.sh
```

### Probar el `.exe` desde Linux con Wine

```bash
sudo apt install wine
WINEDEBUG=-all wine dist/buscatxt-windows-x86_64.exe -V
WINEDEBUG=-all wine dist/buscatxt-windows-x86_64.exe "error*disco" 'Z:\home\usuario\logs' -i
```

Wine muestra las carpetas de Linux en la unidad `Z:`. `WINEDEBUG=-all` oculta los mensajes internos de Wine.

Notas sobre las pruebas con Wine:

- Para comprobar el **codigo de salida**, hazlo desde un `.bat` (`wine cmd /c prueba.bat`). Leido desde bash con `$?`, Wine a veces devuelve 0 aunque el programa haya salido con 1.
- En cmd, `%errorlevel%` escrito en la misma linea que el comando (`b.exe ... & echo %errorlevel%`) se expande antes de ejecutarlo y siempre muestra el valor anterior; usa lineas separadas en un `.bat`.
- Los nombres de archivo con caracteres de control (validos en Linux, prohibidos en Windows) no se pueden abrir desde Wine: el programa muestra un aviso con el nombre neutralizado y continua.
- Los ejemplos de PowerShell se comprobaron con PowerShell 7.6 instalado en Ubuntu (`pwsh`): comillas simples y dobles, `$LASTEXITCODE`, `Select-String`, `Select-Object -First` y `Out-File -Encoding unicode`. Ese PowerShell ejecuta el binario de Linux, o el `.exe` a traves de Wine.
- La version 0.2.0 se comprobo con Wine 10: la salida del `.exe` es identica a la del binario de Linux (comodines, extensiones, UTF-16, Windows-1252, binarios, lineas largas). El modo con colores no se puede comprobar bien con Wine.

## Binarios generados

- `dist/buscatxt-linux-x86_64`: ejecutable para Linux.
- `dist/buscatxt-windows-x86_64.exe`: ejecutable para Windows 10/11 de 64 bits, un solo archivo sin instalador. Solo usa DLLs que vienen con Windows (`KERNEL32`, `msvcrt`, `ntdll`, `api-ms-win-core-synch`, `bcryptprimitives`); esta ultima la usa Rust para generar numeros aleatorios internos de sus tablas, no para cifrar ni para conectarse a nada.

## Licencia

Distribuido bajo la licencia MIT. Ver el archivo [LICENSE](LICENSE).
