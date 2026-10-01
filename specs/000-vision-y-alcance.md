# DBSUAL — visión y alcance inicial

Estado: borrador  
Versión: 2.5  
Última actualización: 2026-09-30

## Visión

DBSUAL es una aplicación de escritorio multiplataforma para gestionar bases de datos en Windows, macOS y Linux. Su experiencia de trabajo toma como referencia la organización de Visual Studio Code: un espacio donde las conexiones, bases de datos, tablas y herramientas de consulta se puedan explorar y utilizar sin cambiar de aplicación. La meta de producto cubre los tres sistemas; la implementación y validación actuales se concentran en Windows y no implican que macOS o Linux ya sean compatibles.

## Personas usuarias

- Desarrolladores que crean, consultan y mantienen bases de datos durante su trabajo.
- Otras personas que trabajan habitualmente con bases de datos y necesitan administrar su estructura y contenido.

## Objetivo de la primera versión

Permitir que una persona, desde cualquiera de los sistemas operativos de escritorio objetivo, configure una conexión a un motor compatible, explore sus bases de datos y tablas, y ejecute las operaciones iniciales de creación, importación, exportación y eliminación de bases de datos que admita ese motor. Las diferencias nativas se resuelven mediante adaptadores por plataforma y no duplicando la lógica de producto.

## Capacidades dentro del alcance inicial

1. Crear y gestionar conexiones a bases de datos.
2. Listar las bases de datos accesibles desde una conexión.
3. Listar las tablas y vistas de una base de datos, y las columnas de cada tabla o vista.
3a. Inspeccionar la estructura detallada de una tabla, incluidas claves, índices, restricciones y relaciones cuando el motor las exponga.
3b. Abrir una tabla en un explorador tabular de sus filas, con lectura paginada independiente del explorador de estructura. Los detalles de lectura y edición se definen en `008-cuadricula-datos.md`.
4. Crear una base de datos, cuando el motor y los permisos lo permitan.
5. Importar desde archivos SQL o CSV, con reglas específicas para cada formato por definir.
6. Exportar a archivos SQL o CSV, con reglas específicas para cada formato por definir.
7. Eliminar una base de datos, con confirmación explícita y una descripción clara del destino de los datos.
8. Preparar, revisar y confirmar todo cambio antes de aplicarlo a una base de datos, con historial y puntos de recuperación para operaciones destructivas.
9. Mantener un historial propio de DBSUAL para revisiones, aplicaciones y restauraciones; ofrecer Git como integración opcional para compartir cambios de estructura y scripts.
10. Escribir y ejecutar consultas SQL desde un editor integrado, con resultados visibles y tratamiento explícito de instrucciones que modifican datos o estructuras.
11. Ver, añadir, editar y eliminar filas de tablas desde una cuadrícula, con modificaciones preparadas en el historial antes de aplicarse.

Las operaciones disponibles deberán reflejar las capacidades y permisos reales de cada conexión. Un fallo no debe dejar la interfaz mostrando como completada una operación que no terminó correctamente.

## Experiencia de producto

- Aplicación de escritorio con una interfaz de trabajo inspirada en un editor: explorador de conexiones y objetos, área principal de trabajo y paneles ajustables.
- Los nombres, acciones y mensajes deben distinguir entre conexión, base de datos y tabla.
- Las acciones destructivas deben exigir confirmación antes de ejecutarse.

Esta sección expresa la dirección del producto; la disposición exacta de los paneles se definirá en un spec de interfaz.

## Stack acordado

| Área | Tecnología |
| --- | --- |
| Aplicación de escritorio | Tauri 2 |
| Interfaz | React, TypeScript, Vite |
| Componentes y estilos | Tailwind CSS, Radix UI / shadcn, Lucide |
| Estado | Zustand |
| Paneles ajustables | react-resizable-panels |
| Editor SQL | Monaco Editor |
| Core | Rust, Tokio, Serde |
| Acceso a datos | SQLx |
| Comunicación interfaz-core | Tauri Commands / IPC |
| Credenciales | Almacén seguro nativo del sistema operativo, detrás de una interfaz de plataforma |
| Configuración e historial local | SQLite para metadatos; artefactos y respaldos cifrados en archivos separados |
| Pruebas | Vitest, Playwright |
| Integración y entrega | GitHub Actions |

El editor y la ejecución SQL forman parte del MVP. Su comportamiento se define en `007-editor-sql.md`.

## Motores y formatos del MVP

- Motores: MySQL, MariaDB, PostgreSQL y SQLite.
- Plataformas objetivo del producto: Windows, macOS y Linux. Windows es la primera plataforma de implementación y la única cuya aplicación nativa está validada actualmente; consultar `019-plataformas.md` para estado y puertas de soporte.
- Orden de implementación: MySQL, MariaDB, PostgreSQL y SQLite.
- Versiones iniciales para pruebas de los servidores MySQL/MariaDB: MySQL 8.0 y 8.4; MariaDB 10.6, 10.11 y 11.4.
- Formatos de importación y exportación: SQL para una base de datos completa, con estructura y datos; CSV por tabla.
- El alcance de CSV será por tabla; queda pendiente definir si se permite seleccionar varias tablas en una sola operación.
- En SQLite, una base de datos corresponde a un archivo. Las operaciones de crear y eliminar deberán describirse específicamente para ese caso en el spec funcional.

## Decisiones pendientes para concretar el MVP

1. Selección múltiple de tablas para CSV y matriz exacta de SQL importable por motor; la política general de conflictos y resultados parciales se define en el spec 015.
2. Detalles de compatibilidad de certificados y claves para TLS y SSH; ambos mecanismos, incluidos certificados de cliente TLS y agente SSH, forman parte del MVP.
3. Definir los mecanismos de respaldo por motor y el margen de espacio libre requerido. La retención será manual y la sincronización entre equipos queda fuera del primer MVP. Las contraseñas y secretos se almacenarán en el almacén seguro nativo de cada sistema operativo, nunca en SQLite.

## Siguientes specs propuestos

1. `001-conexiones.md`: alta, edición, prueba, uso y eliminación de conexiones; manejo de credenciales y errores.
2. `002-explorador.md`: listado y actualización de bases de datos, tablas, vistas y columnas.
3. `003-operaciones-bases-de-datos.md`: creación y eliminación, permisos y confirmaciones.
4. `004-importacion-exportacion.md`: formatos, progreso, errores y consistencia.
5. `005-interfaz-de-trabajo.md`: navegación, paneles y estados de la aplicación.
6. `006-control-de-cambios.md`: preparación, revisión, historial, aplicación y recuperación de cambios en bases de datos.
7. `007-editor-sql.md`: edición, ejecución y resultados de consultas SQL.
8. `008-cuadricula-datos.md`: vista y edición de filas desde una cuadrícula.
9. `009-arquitectura.md`: límites entre interfaz, core, motores, historial y respaldos.
10. `010-respaldos-y-recuperacion.md`: garantías de recuperación por cambio y por copia completa.
11. `011-respaldos-mysql-mariadb.md`: primer proveedor de copia y restauración por motor.
12. `012-respaldos-postgresql.md`: proveedor de copia y restauración PostgreSQL.
13. `013-respaldos-sqlite.md`: proveedor de copia y restauración SQLite.
14. `014-claves-y-artefactos-cifrados.md`: protección local de respaldos y recuperación mediante frase o archivo de clave.
15. `015-conflictos-y-aplicacion.md`: comprobación de cambios externos, aplicación por pasos y tratamiento de resultados parciales.
16. `016-plan-sdd-mvp.md`: orden de entregas, primer corte funcional y pruebas requeridas antes de ampliar el alcance.
17. `017-base-aplicacion-windows.md`: alcance y pruebas de la primera entrega de la aplicación Tauri para Windows.
18. `019-plataformas.md`: alcance multiplataforma, capacidades nativas y criterios para habilitar Windows, macOS y Linux.
19. `020-apariencia-y-temas.md`: preferencias visuales, temas completos y colores personalizados.

Cada spec funcional deberá incluir casos de uso, reglas, estados de error y criterios de aceptación verificables antes de implementar esa función.
