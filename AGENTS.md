# Instrucciones para agentes — DBSUAL

Este archivo se aplica a todo el repositorio. Está dirigido a agentes que diseñan, implementan, revisan o documentan DBSUAL. Trabaja en español al comunicar decisiones, resultados y pendientes a la persona usuaria. Conserva en inglés los identificadores de código y los nombres oficiales de tecnologías.

## Propósito y fuentes de verdad

DBSUAL es un gestor de bases de datos de escritorio multiplataforma para Windows, macOS y Linux, con una organización visual inspirada en Visual Studio Code. Está dirigido a desarrolladores y otras personas que trabajan con bases de datos. La primera versión contempla MySQL, MariaDB, PostgreSQL y SQLite, en ese orden de implementación. Windows es el entorno implementado y validado hasta ahora; no afirmes soporte de macOS o Linux hasta superar los criterios del spec 019.

- [Visión y alcance](specs/000-vision-y-alcance.md): producto, motores y stack acordado.
- [Plataformas](specs/019-plataformas.md): objetivo Windows/macOS/Linux, estado real por sistema operativo y puertas para declarar soporte.
- [Apariencia y temas](specs/020-apariencia-y-temas.md): paletas, tema claro, personalización de colores y validación visual.
- [Plan SDD](specs/016-plan-sdd-mvp.md): secuencia de entregas, puertas de salida y evidencia requerida.
- [Specs funcionales](specs/001-conexiones.md): requisitos y criterios de aceptación por capacidad. Lee también el spec específico del área que vas a modificar.
- [Base Windows](specs/017-base-aplicacion-windows.md): evidencia específica de la primera plataforma; no define el alcance total del producto. [Recorrido MySQL](specs/018-primer-recorrido-mysql.md): estado y decisiones de la entrega actual.
- [README](README.md): preparación del entorno y comandos vigentes.

El código y las pruebas muestran lo implementado; un spec puede describir trabajo futuro. Si hay una discrepancia, comprueba el comportamiento, registra la decisión en el spec correspondiente y explica el alcance real. No declares una entrega completa solo porque compila.

## Estado actual

- **Entrega 0:** base Tauri para Windows, preferencias en SQLite local y sesión segura de Bienvenida/Consulta SQL, incluso sin pestañas. Bienvenida se puede cerrar y reabrir desde Inicio; el área vacía ofrece accesos de recuperación. Configuración contiene la administración del depósito fuera de Inicio. Rust valida la sesión vacía y Playwright verifica los cierres y navegación en preview. Parcial: recuperación nativa de la sesión al cerrar/reabrir Windows y revisión del producto siguen pendientes.
- **Entrega 1:** formulario y ciclo de vida de conexiones MySQL y MariaDB, contraseña en credenciales de Windows, TLS directo, SSH con aprobación explícita de host y explorador parcial de bases, tablas, vistas y columnas. PostgreSQL ya cuenta con conexión directa separada, puerto 5432, TLS VerifyFull, comandos de probar/abrir/desconectar, listado de bases y catálogo de tablas, vistas y columnas agrupadas por esquema. Una integración PostgreSQL 16 verificó objetos homónimos en dos esquemas, una vista y metadatos de columnas; los comandos Tauri están integrados; falta probarlos en la ventana nativa, TLS real y cuenta de permisos mínimos. La cuadrícula de solo lectura por tabla y el editor también admiten `SELECT` PostgreSQL en una transacción de solo lectura y páginas acotadas, comprobadas contra PostgreSQL 16; faltan recorrido nativo, cancelación, cambios, historial y respaldo. El formulario oculta Procesos, Usuarios y Variables para PostgreSQL. La integración MariaDB pasó en 10.6.28, 10.11.19 y 11.4.13 para conexión, lectura, catálogo, estructura y metadatos de servidor. El árbol MySQL incluye Procesos, Usuarios y Variables bajo demanda; MySQL 8.4.11 verificó sus consultas seguras. Las tablas ofrecen cuadrícula de solo lectura con lectura paginada, orden y filtro remoto básico, además de ficha de estructura con índices y restricciones, abierta en pestaña amplia de trabajo. Playwright comprueba la navegación de esa ficha; la ventana nativa aún requiere recorrido. Integraciones separadas probaron filtros enlazados y páginas estables en MySQL 8.4.11 y MariaDB 10.6.28, 10.11.19 y 11.4.13. Falta el recorrido nativo, permisos mínimos, claves TLS de cliente con frase de paso, SSH real y matriz TLS/permisos. MariaDB no tiene respaldo habilitado para esta cuadrícula.
- **Entrega 2:** Monaco local y lectura `SELECT` con límites están implementados parcialmente; paginación y cancelación explícita se comprobaron contra MySQL 8.4.11. Conexión/lectura SQL acotada también pasó en MariaDB 10.6.28, 10.11.19 y 11.4.13. Al desconectar o quitar una conexión, el core cancela las lecturas registradas y la interfaz invalida sus respuestas pendientes. Falta verificar la cancelación con consultas largas reales y los lotes de 200 filas advierten sobre cambios concurrentes.
- **Entrega 3:** depósito local, importación de archivo de clave con verificación de frase nueva, preparación/revisión/confirmación local de un plan SQL cifrado. Captura y restauración parcial MySQL/MariaDB están conectadas a IPC e historial; restaurar en una base nueva pasa por preparar, revisar, confirmar y aplicar con plan cifrado ligado al punto y al servidor; se autentica el artefacto antes de crear una base nueva. El round-trip parcial de tablas, FK, vista y trigger pasó contra MySQL Community 8.0.46 y 8.4.11; dos triggers en orden de nombres inverso conservaron `ACTION_ORDER` en 8.4.11. La cobertura permanece `captured` y no habilita cambios destructivos. Una edición localizada de celda MySQL con compensación cifrada está implementada para columnas no clave de tablas InnoDB con PK, sin triggers y con tipos admitidos. Tests Windows invocan comandos de producción por Tauri MockRuntime; recorridos completos de importación y edición/reversión pasaron con MySQL 8.4.11 desechable (preparar y confirmar no escriben; solo aplicar muta). Esto no acredita el recorrido en ventana nativa. Falta verificar la aplicación/reversión desde la ventana Tauri nativa con MySQL real, cubrir rutinas, funciones y eventos, matriz completa de versiones/permisos, restauración completa y recuperación de claves exportadas; la aplicación general de cambios y la reconciliación posterior aún no están implementadas.
- **Entrega 4:** exportación CSV de una tabla MySQL con separador y marcador NULL configurables. La importación CSV implementa comandos Tauri, plan cifrado, revisión de historial, lote transaccional y compensación; la integración del servicio pasó contra MySQL Community 8.0.46 comprobando atomicidad, conflicto externo y reversión. El flujo de preparar/confirmar/aplicar/preparar compensación/confirmar/aplicar compensación también pasó por los comandos de producción mediante Tauri MockRuntime contra MySQL Community 8.4.11 desechable. La UI muestra la vista previa, pero mantiene la mutación deshabilitada hasta recorrerla desde la ventana Tauri nativa en Windows. La prueba de interfaz actual verifica que no se envíen llamadas de mutación. Las integraciones reales de crear base, leer/filtrar/paginar y editar/compensar/conflicto pasaron contra MySQL 8.0.46; edición e inserción también pasaron previamente contra 8.4.11. Sigue pendiente el recorrido nativo de aplicación/reversión y el refresco del explorador. También faltan respaldo/restauración completa, eliminación de bases, edición estructural, eliminación ordinaria de filas y exportación/importación SQL completa.
- **Plataformas:** Windows es la única plataforma con compilación de aplicación y paquetes MSI/NSIS comprobados. Ambos instaladores se instalaron y desinstalaron en carpetas temporales; falta arrancar la aplicación instalada, probar actualización y verificar limpieza de datos en Windows limpio. macOS y Linux son objetivos del producto todavía pendientes de configuración, build nativo, almacenes de secretos, instaladores y matriz de verificación, conforme al spec 019. No presentes evidencia Windows como evidencia multiplataforma.
- Los recorridos que hoy pueden modificar bases son acotados: crear una base MySQL vacía, restaurar la cobertura parcial MySQL/MariaDB en una base nueva y editar una celda MySQL elegible tras historial, revisión, confirmación y recuperación compensatoria. No se reemplazan bases existentes. La creación y edición se verificaron a nivel de servicio con MySQL 8.4.11; edición y reversión además pasaron por comandos Tauri MockRuntime con MySQL 8.4.11. La restauración parcial pasó con MySQL 8.0.46/8.4.11 y MariaDB en sus tres ramas objetivo, pero sus rutas Tauri nativas no se recorrieron en esta sesión. SQL modificador general, importación aplicada y operaciones destructivas siguen bloqueadas. SQLite sigue en solo lectura; PostgreSQL ofrece conexión, catálogo por esquema, metadatos, editor SQL de solo lectura y cuadrícula de solo lectura probada con PostgreSQL 16.

## Forma de trabajo SDD

1. Localiza el spec y los criterios de aceptación de la capacidad. Identifica decisiones abiertas y dependencias antes de editar.
2. Define el recorrido observable y el contrato entre React y Rust: entradas, salidas, estados, códigos de error y persistencia.
3. Implementa una porción vertical utilizable: interfaz, IPC, core, acceso al motor y almacenamiento que ese recorrido necesite.
4. Verifica criterios en el sistema operativo que se declara compatible. Windows es la plataforma de referencia actual; para ampliar soporte, sigue la matriz y las puertas del spec 019. Para comportamiento de MySQL, permisos, TLS, SSH o recuperación, utiliza un servidor desechable y registra sistema operativo, versión/configuración del motor y configuración relevante. Una prueba de interfaz web no sustituye la prueba nativa o con una base real.
5. Actualiza el spec con decisiones, evidencia y limitaciones. Mantén como pendiente cualquier criterio que no haya sido comprobado.

Evita añadir pruebas que repitan la implementación sin comprobar una propiedad observable. Si falta infraestructura externa, registra el criterio como no verificado; no simules un resultado exitoso.

## Reglas del producto que no se deben romper

- **Toda modificación de una base externa** pasa por preparar → revisar → confirmar → aplicar en el historial de DBSUAL. Solo están habilitados estos recorridos acotados documentados arriba: crear una base MySQL vacía, restaurar cobertura parcial en una base nueva, editar una celda MySQL elegible e insertar una fila MySQL elegible; edición e inserción tienen compensación. La eliminación ordinaria, SQL modificador general y operaciones destructivas siguen bloqueadas. No habilites otra operación hasta que identificación del destino, detección de conflictos y recuperación requerida estén implementadas y probadas. Consulta los specs 006, 010, 014 y 015.
- Preparar o confirmar una revisión no ejecuta cambios en la base. El resultado de una aplicación, error o restauración debe reflejar lo que realmente ocurrió.
- El historial de bases es propio de DBSUAL. Git versiona este repositorio de código; una integración opcional con Git puede compartir scripts o cambios de estructura, pero no reemplaza el historial operativo.
- Las operaciones destructivas requieren confirmación y un punto de recuperación válido. En SQLite, eliminar una base significa borrar su archivo solo después de confirmar y cumplir las condiciones de recuperación.
- Nunca guardes contraseñas, claves, frases de paso, consultas con datos sensibles o resultados en el SQLite de metadatos, Zustand, logs, errores IPC o Git. Usa el almacén seguro de credenciales del sistema operativo mediante el adaptador de plataforma definido en los specs 014 y 019; no uses almacenamiento en claro como alternativa cuando el almacén nativo no esté disponible.
- Un fallo de conexión, permisos, carga o escritura debe mostrarse como error. No lo conviertas en una lista vacía ni en éxito. No abras conexiones automáticamente al restaurar una sesión.
- TLS configurado como requerido debe verificar la identidad del servidor y fallar cerrado. SSH exige verificar la clave de host; no aceptes claves nuevas o cambiadas automáticamente. No rebajes la verificación al combinar SSH y TLS sin una decisión documentada.
- Distingue siempre conexión, base de datos, tabla y vista en nombres, mensajes y acciones.

## Arquitectura y ubicación del código

| Área | Ubicación y responsabilidad |
| --- | --- |
| Entrada visual y disposición | `src/App.tsx`, `src/styles.css`; ventana, paneles y navegación. |
| Conexiones y explorador | `src/ConnectionWorkspace.tsx`; formularios, estados y árbol cargado por nivel. |
| Contrato frontend | `src/ipc.ts`; tipos y llamadas a Tauri. No devuelve secretos tras guardarlos. |
| Estado visual | `src/store.ts`; Zustand mantiene preferencias en memoria y persiste mediante IPC. |
| Comandos y estado Tauri | `src-tauri/src/lib.rs`; registra comandos, estado compartido y errores estables. |
| Conexiones MySQL | `src-tauri/src/connections.rs`; validación, credenciales a través del almacén nativo, pools, TLS y metadatos. |
| SQLite local | `src-tauri/src/storage.rs`; rutas Tauri, migraciones transaccionales y estado no secreto. |
| Pruebas de interfaz | `src/**/*.test.ts`; Vitest. `tests/e2e/`; Playwright. |
| Automatización | `.github/workflows/ci.yml`; Windows ejecuta la suite y empaqueta; jobs macOS/Linux añadidos para build y `cargo check`, pendientes de ejecución. El spec 019 define las puertas restantes. |

Stack acordado: Tauri 2, React, TypeScript, Vite, Tailwind CSS, Radix UI/shadcn, Lucide, Zustand, react-resizable-panels, Monaco, Rust, Tokio, Serde y SQLx. Algunas bibliotecas previstas todavía no están instaladas porque su capacidad no se ha implementado. Respeta los archivos lock y las versiones fijadas en `.nvmrc` y `rust-toolchain.toml`.

## Contratos y almacenamiento

- Añade los comandos Tauri en `lib.rs` y su wrapper tipado en `src/ipc.ts` en el mismo cambio. Usa nombres `snake_case` en comandos y `camelCase` en JSON/TypeScript.
- Valida en Rust cada dato recibido por IPC, aunque el formulario también lo valide. Mantén códigos de error estables y mensajes sin secretos.
- Resuelve el directorio de datos local mediante la API de rutas de Tauri. No construyas rutas a partir del nombre de usuario Windows.
- Introduce cambios de esquema SQLite mediante migraciones ordenadas y transaccionales. Rechaza una versión de esquema más nueva sin intentar degradarla ni crear otro historial vacío.
- Las conexiones guardadas conservan un ID estable al cambiar de nombre. La contraseña se obtiene del almacén seguro nativo del sistema solo cuando hace falta conectar y nunca se devuelve en `SavedConnection`. Windows está validado; macOS/Linux siguen pendientes según 019.
- La vista previa web puede mostrar la interfaz y validar formularios; las acciones que requieren el core deben informar que solo funcionan en la aplicación de escritorio.

## Entorno y comprobaciones

La configuración comprobada localmente es Windows y requiere Node 24, Rust 1.98 MSVC, Visual Studio Build Tools, Windows SDK y WebView2. macOS y Linux requieren toolchains y bibliotecas nativas propias de Tauri; consulta el spec 019 antes de preparar esos entornos. En Windows, desde la raíz:

```powershell
npm ci
npx playwright install chromium
npm run tauri dev
```

Comprobaciones disponibles:

```powershell
npm run build
npm run test:unit
npm run test:e2e
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo test --manifest-path src-tauri/Cargo.toml --locked
```

Ejecuta las comprobaciones pertinentes al cambio; no afirmes cobertura de una conexión real si solo pasaron compilación y pruebas locales. Evita usar credenciales o bases personales para pruebas; utiliza datos y servidores desechables.

## Git y colaboración

Revisa `git status` antes de editar y conserva los cambios existentes de otras personas o agentes. Haz commits que describan una porción coherente y deja el árbol limpio cuando corresponda; no publiques ni cambies el remoto sin una instrucción de la persona usuaria. Si delegas tareas, separa archivos o responsabilidades para evitar ediciones simultáneas y prioriza modelos de menor costo, como GPT-6 Luna, según la preferencia expresada por la persona usuaria.

Al informar un avance, indica qué cambió, qué se verificó y qué criterios siguen pendientes, con enlaces a archivos del repositorio. Mantén actualizados el README y el spec afectado cuando cambie el alcance o el estado de una entrega.
- **Apariencia:** las seis paletas predeterminadas tienen prueba de tokens desde `src/styles.css`: texto normal/secundario ≥ 4.5:1 y acento ≥ 3:1 sobre cuatro superficies. Vitest pasa 7/7, build pasa y el E2E de apariencia 1/1 en preview; falta auditar overrides por componente y revisar la ventana nativa, sin declarar legibilidad multiplataforma.
- **E2E:** Playwright producción preview 25/25 en Windows el 2026-09-30. Se actualizó el test del área vacía para usar «Nueva consulta SQL» accesible en vez de un selector antiguo; esto no es evidencia del recorrido en ventana nativa.
