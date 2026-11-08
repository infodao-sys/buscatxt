# Code signing policy / Politica de firma de codigo

Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by [SignPath Foundation](https://signpath.org)

## English

- **Signed files**: only `buscatxt-windows-x86_64.exe` (the Windows executable of buscatxt), built from this repository's source code by GitHub Actions on GitHub-hosted runners ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)), compiled with MSVC. Nothing else is signed: buscatxt has no third-party code or dependencies.
- **Signing process**: a signing request is sent only when the workflow is started manually with the `firmar` option, and each request is approved by hand in SignPath before signing. Signed executables are published on the [Releases](https://github.com/infodao-sys/buscatxt/releases) page together with their SHA-256 checksums.
- **Team roles**:
  - Committers and reviewers: [infodao-sys](https://github.com/infodao-sys)
  - Approvers: [infodao-sys](https://github.com/infodao-sys)
- **Contributions**: changes from people outside the team (pull requests) are reviewed by a team member before being merged. All team members use multi-factor authentication on GitHub and SignPath.
- **Privacy policy**: this program will not transfer any information to other networked systems. buscatxt only reads the files and folders that the user indicates; it does not open network connections, does not collect or send data, and does not run other programs.
- **Installation and removal**: the install scripts in `scripts/` only copy the executable to a user folder and add it to the `PATH`; the same scripts uninstall it (`--desinstalar` / `-Desinstalar`).

## Espanol

- **Archivos firmados**: solo `buscatxt-windows-x86_64.exe` (el ejecutable de Windows de buscatxt), compilado con MSVC a partir del codigo fuente de este repositorio por GitHub Actions en servidores de GitHub ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)). No se firma nada mas: buscatxt no incluye codigo ni dependencias de terceros.
- **Proceso de firma**: la solicitud de firma solo se envia cuando el flujo se lanza a mano con la opcion `firmar`, y cada solicitud se aprueba a mano en SignPath antes de firmar. Los ejecutables firmados se publican en [Releases](https://github.com/infodao-sys/buscatxt/releases) junto con sus huellas SHA-256.
- **Funciones del equipo**:
  - Autores y revisores: [infodao-sys](https://github.com/infodao-sys)
  - Aprobadores: [infodao-sys](https://github.com/infodao-sys)
- **Contribuciones**: los cambios de personas ajenas al equipo (pull requests) los revisa un miembro del equipo antes de incorporarlos. Todos los miembros del equipo usan autenticacion en dos pasos en GitHub y en SignPath.
- **Privacidad**: este programa no transfiere ninguna informacion a otros sistemas en red. buscatxt solo lee los archivos y carpetas que indica el usuario; no abre conexiones de red, no recopila ni envia datos y no ejecuta otros programas.
- **Instalacion y desinstalacion**: los scripts de `scripts/` solo copian el ejecutable a una carpeta del usuario y lo agregan al `PATH`; los mismos scripts lo desinstalan (`--desinstalar` / `-Desinstalar`).
