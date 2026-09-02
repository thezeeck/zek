1. Dependencias declaradas (needs: [build, test]): paralelismo automático derivado del grafo, más claro que parallel: true y soporta encadenamientos.
2. --step <nombre> / --until <nombre>: correr solo una parte del flujo (útil para depurar sin re-ejecutar todo).
3. Retry con backoff exponencial (retry_backoff: true) además del retry_delay fijo.
4. .env y secretos de gestores (cargar env_file: .env, o referenciar 1Password/sops) para no exponer tokens en YAML.
5. Variables de flujo (vars: a nivel de flujo, referenciables con {{vars.x}} y sobreescribibles por CLI).
6. zek graph: imprimir el DAG del flujo (dependencias, paralelismo, goto) para visualizar la estructura.
7. Notificaciones (notify: slack|discord|telegram al terminar con éxito/fallo).
 8. Historial (zek history / zek logs): consultar ejecuciones pasadas desde el log.
 9. Exportar a CI (zek export github): generar un workflow de GitHub Actions a partir de un flujo.
10. Modo watch (zek watch <flujo>): re-ejecutar ante cambios de archivos (dev loop).
11. Parseo de salida de command (output_format: json en steps command) para usar {{steps.x.json.campo}}.
12. Cache de steps (saltar un comando si no cambiaron sus entradas, estilo CI).
