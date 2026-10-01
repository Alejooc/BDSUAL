# Spec 019 — Soporte de plataformas de escritorio

Estado: objetivo acordado; compatibilidad actual solo Windows
Versión: 0.1
Última actualización: 2026-09-30
Depende de: [000 — Visión](000-vision-y-alcance.md), [009 — Arquitectura](009-arquitectura.md), [014 — Claves y artefactos](014-claves-y-artefactos-cifrados.md), [016 — Plan SDD](016-plan-sdd-mvp.md), [017 — Base Windows](017-base-aplicacion-windows.md)

## Objetivo y alcance

DBSUAL es un producto de escritorio para **Windows, macOS y Linux**. La funcionalidad común (React, contratos IPC, reglas de conexiones/motores, consultas, cuadrícula e historial) debe mantenerse compartida. Las diferencias del sistema se implementan detrás de adaptadores nativos pequeños, no mediante bifurcaciones de la experiencia de producto.

Este objetivo no declara soporte anticipado. Según la evidencia actual, Windows es la única plataforma con build, prueba nativa y empaquetado registrados. macOS y Linux son targets pendientes. La guía Windows del spec 017 describe evidencia de una plataforma, no reduce el alcance del producto.

El MVP es de escritorio. Android, iOS, WebView en navegador como producto y sincronización del estado local entre sistemas quedan fuera de este spec.

## Matriz de estado

| Sistema | Estado | Evidencia que existe | Pendiente para considerarlo compatible |
| --- | --- | --- | --- |
| Windows | Plataforma de referencia, parcialmente validada | Build Tauri x64; artefactos NSIS/MSI verificados estáticamente; pruebas Rust/UI y algunos recorridos nativos descritos en specs 017–018 | Recorrido instalado en entorno limpio, ciclo de actualización/desinstalación, firma y revisión final del producto |
| macOS | Target planificado; no compatible aún | Selección del backend Keychain configurada en Cargo; job CI añadido, pendiente de ejecución | Build nativo, bundle instalable, recorrido nativo, permisos y prueba de recuperación |
| Linux | Target planificado; no compatible aún | Selección de Secret Service configurada en Cargo; job CI añadido, pendiente de ejecución; spike SQLCipher aislado que no prueba DBSUAL | Build nativo, dependencias WebKitGTK, distribución acordada, sesión Secret Service, recorrido nativo y recuperación |

La arquitectura del repositorio fija Node 24 y Rust 1.98; el toolchain nativo y las dependencias del WebView varían por sistema. Versiones mínimas de OS, distribuciones Linux, arquitecturas (x64/ARM64) y formatos de distribución son decisiones pendientes; se deben resolver antes de publicar builds de cada plataforma.

## Límites por plataforma

- **Interfaz y core:** componentes React, tipos IPC, reglas de validación, adaptadores SQL y formato portable de artefactos no deben depender de rutas, APIs de credenciales ni ejecutables propios de un OS.
- **Secretos:** usar el almacén seguro nativo del perfil actual (Windows Credential Manager, macOS Keychain, Secret Service en Linux). El backend se selecciona por target de Cargo y se accede a través de una frontera común. La falta de un almacén disponible falla con error claro; no hay fallback persistente en claro. Las copias recuperables usan los formatos y vías portables del spec 014.
- **Rutas y archivos:** obtener rutas con Tauri API, respetar permisos privados, separadores, nombres y reglas de reemplazo atómico propias del OS. No derivar rutas del nombre del usuario ni asumir que un filesystem soporta hard links.
- **Diálogos y permisos:** selección de bases SQLite, importación/exportación y archivos de claves usan diálogos nativos de Tauri con filtros y permisos mínimos. Se prueban rutas con espacios, Unicode, nombres conflictivos y cancelación.
- **TLS y SSH:** probar raíces de confianza, certificados, ubicación/socket del agente SSH y mensajes de error en cada sistema. No asumir el agente OpenSSH de Windows en macOS/Linux y no bajar la verificación TLS/host para unificar plataformas.
- **Procesos auxiliares y respaldos:** cualquier ejecutable externo debe resolverse por target, validarse en versión/procedencia/hash cuando corresponda y manejar invocación sin exponer secretos en argumentos. Los mecanismos de los proveedores por motor se detallan en sus specs.
- **Ventanas e interacción:** navegación, foco, menús contextuales, selección de archivos, desplazamiento, escalado y cierre se verifican bajo los WebView y decoraciones nativas objetivo. La vista previa web no sustituye una prueba de escritorio.
- **Dependencias nativas:** explicar e instalar los prerrequisitos de compilación y ejecución por target. Una falla por dependencia nativa no se convierte en una falsa lista vacía ni en éxito parcial.

## CI, paquetes y soporte

- CI conserva los instaladores Windows existentes y tiene jobs nativos de macOS y Linux para build del frontend/core; falta ejecutarlos y extender la matriz a pruebas adecuadas y paquetes antes de declarar soporte.
- Las pruebas que acceden al almacén de secretos, diálogos o WebView se ejecutan en runners con el servicio/sesión necesario o en una verificación manual reproducible. No se simula como éxito una operación nativa que el runner no proporciona.
- Cada artefacto declara OS, arquitectura, formato y versión. La distribución, firma, actualización, instalación y desinstalación se validan por separado; compilar en CI no es evidencia de que un instalador funcione.
- README, guía de usuario, CI y mensaje de descarga solo anuncian los sistemas/arquitecturas que hayan cruzado la puerta de soporte. Los targets aún en desarrollo se identifican como tales.

## Criterios para declarar soporte en un sistema

Para declarar una plataforma compatible, todas estas condiciones deben tener evidencia reproducible para cada arquitectura publicada:

1. El frontend y el core compilan en runner nativo, con dependencias bloqueadas y sin código condicionado del otro sistema.
2. Se construye e instala el paquete acordado en una instalación limpia; la aplicación abre, usa rutas de datos correctas, actualiza desde una versión previa y se desinstala según política documentada.
3. La interfaz completa permite guardar/cerrar sesión, explorar/conectar y consultar una base de prueba; errores de permisos, certificados y archivos se muestran en la zona correspondiente.
4. El backend seguro permite guardar, leer, actualizar y borrar una credencial de prueba; queda comprobado que una indisponibilidad no escribe secretos en disco sin protección.
5. Una clave portable recupera un artefacto cifrado sin depender del almacén, perfil ni sistema operativo donde fue creado. La prueba de frase/archivo no se limita a probar el cifrado unitario.
6. Selección nativa de archivos, exportación, cierre, scroll/foco y escala de ventana pasan un smoke test de la aplicación empaquetada.
7. CI ejecuta build/formato/tests adecuados y publica artefactos identificados de forma inequívoca; las limitaciones por OS/arquitectura permanecen visibles.

Un criterio no comprobado deja esa plataforma como **en desarrollo**, aunque compile o se haya probado solo desde el navegador.

## Plan de habilitación

1. Conservar Windows como plataforma de referencia mientras se cierra el resto de la matriz; la visión, arquitectura y guías generales ya declaran Windows/macOS/Linux como alcance.
2. Configurar Keychain macOS y Secret Service Linux detrás del API común; comprobar sus builds nativos y definir el comportamiento de sesiones Linux sin Secret Service.
3. Ejecutar los jobs macOS/Linux añadidos a CI y corregir errores de compilación/recursos sin relajar las reglas de secretos ni recuperación.
4. Elegir versiones, arquitecturas y paquetes objetivo; generar y validar instaladores nativos.
5. Ejecutar recorridos nativos de conexión, SQLite, TLS/SSH, depósito/recuperación y cierre/actualización; actualizar la matriz con versiones y resultados.

## Decisiones pendientes

1. Versión mínima soportada de Windows y macOS.
2. Distribuciones/versiones Linux y formatos iniciales de paquete (por ejemplo, `.deb`, AppImage o Flatpak); declarar explícitamente las dependencias de WebKitGTK y Secret Service.
3. Arquitecturas de cada sistema (x64, ARM64) y targets de Rust.
4. Firma de código, notarización macOS, firma/repositorio de paquetes Linux, canal de actualización y retención de versiones.
5. Comportamiento de la aplicación cuando no existe un almacén seguro desbloqueado, especialmente en sesiones headless de Linux.

## Referencias técnicas

- [Tauri 2 — prerrequisitos de compilación por sistema operativo](https://v2.tauri.app/start/prerequisites/).
- [keyring 3.6.3 — features y backends nativos](https://docs.rs/crate/keyring/3.6.3/source/Cargo.toml).
