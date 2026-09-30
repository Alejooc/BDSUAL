# DBSUAL

**DBSUAL** es una aplicación de escritorio para Windows que reúne conexiones, exploración de objetos y consultas SQL en un espacio de trabajo claro. Está construida con Tauri, React y Rust.

> **Estado:** proyecto en desarrollo. La versión actual está orientada a evaluación y pruebas; algunas funciones, motores y recorridos aún tienen cobertura parcial. No se recomienda usarla como única herramienta de respaldo o recuperación de bases de datos.

## Funciones

- Conexión a MySQL, MariaDB y PostgreSQL; apertura de archivos SQLite existentes en modo de solo lectura.
- Exploración de bases, esquemas, tablas, vistas y columnas, según el motor.
- Cuadrículas de datos y consultas `SELECT` con límites, paginación y controles de solo lectura donde están disponibles.
- Conexiones seguras con almacenamiento de contraseñas en el Administrador de credenciales de Windows.
- Herramientas de historial y recuperación parcial para algunos recorridos MySQL y MariaDB.

La cobertura y las operaciones disponibles varían por motor. PostgreSQL y SQLite se mantienen en modo de solo lectura; la captura y restauración MySQL/MariaDB son parciales. Revisa cada operación antes de usarla con datos importantes.

## Requisitos

- Windows 10/11 x64 con WebView2 Runtime.
- Node.js 24 (versión fijada en `.nvmrc`).
- Rust 1.98 con el target MSVC.
- Visual Studio Build Tools y Windows SDK.

## Desarrollo local

En PowerShell, desde la raíz del repositorio:

```powershell
npm ci
npx playwright install chromium
npm run tauri dev
```

Para compilar y ejecutar las comprobaciones:

```powershell
npm run build
npm run test:unit
npm run test:e2e
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo test --manifest-path src-tauri/Cargo.toml --locked
```

La integración continua valida estos pasos y genera instaladores MSI y NSIS de prueba como artefactos temporales. Los instaladores todavía no están firmados para distribución.

## Seguridad y datos

Las contraseñas de conexiones se almacenan en el Administrador de credenciales de Windows. DBSUAL guarda preferencias y metadatos no secretos en su directorio de datos local. No incluyas credenciales, volcados de bases de datos ni información sensible en incidencias o cambios del repositorio.

## Licencia

DBSUAL se distribuye bajo la licencia MIT. Consulta [`LICENSE`](LICENSE). Las dependencias conservan sus propias licencias.

## Contribuir

Las contribuciones son bienvenidas mediante issues y pull requests. Antes de enviar cambios, ejecuta las comprobaciones pertinentes y describe claramente qué se probó y qué queda pendiente.
