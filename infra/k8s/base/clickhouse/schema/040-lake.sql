CREATE TABLE IF NOT EXISTS dusk.lake_ledger_parquet ON CLUSTER dusk AS dusk.ledger
ENGINE = S3(lake, filename = 'ledger/dt=*/*.parquet', format = 'Parquet');

CREATE OR REPLACE VIEW dusk.lake_ledger ON CLUSTER dusk
DEFINER = default SQL SECURITY DEFINER
AS SELECT *, toDate(extract(CAST(_path AS String), '/dt=([0-9]{4}-[0-9]{2}-[0-9]{2})/')) AS dt, CAST(_path AS String) AS lake_path
FROM dusk.lake_ledger_parquet;

CREATE TABLE IF NOT EXISTS dusk.lake_connections_parquet ON CLUSTER dusk AS dusk.connections
ENGINE = S3(lake, filename = 'connections/dt=*/*.parquet', format = 'Parquet');

CREATE OR REPLACE VIEW dusk.lake_connections ON CLUSTER dusk
DEFINER = default SQL SECURITY DEFINER
AS SELECT *, toDate(extract(CAST(_path AS String), '/dt=([0-9]{4}-[0-9]{2}-[0-9]{2})/')) AS dt, CAST(_path AS String) AS lake_path
FROM dusk.lake_connections_parquet;

CREATE TABLE IF NOT EXISTS dusk.lake_enrollments_parquet ON CLUSTER dusk AS dusk.enrollments
ENGINE = S3(lake, filename = 'enrollments/dt=*/*.parquet', format = 'Parquet');

CREATE OR REPLACE VIEW dusk.lake_enrollments ON CLUSTER dusk
DEFINER = default SQL SECURITY DEFINER
AS SELECT *, toDate(extract(CAST(_path AS String), '/dt=([0-9]{4}-[0-9]{2}-[0-9]{2})/')) AS dt, CAST(_path AS String) AS lake_path
FROM dusk.lake_enrollments_parquet;

CREATE TABLE IF NOT EXISTS dusk.lake_process_results_parquet ON CLUSTER dusk AS dusk.process_results
ENGINE = S3(lake, filename = 'process_results/dt=*/*.parquet', format = 'Parquet');

CREATE OR REPLACE VIEW dusk.lake_process_results ON CLUSTER dusk
DEFINER = default SQL SECURITY DEFINER
AS SELECT *, toDate(extract(CAST(_path AS String), '/dt=([0-9]{4}-[0-9]{2}-[0-9]{2})/')) AS dt, CAST(_path AS String) AS lake_path
FROM dusk.lake_process_results_parquet;

CREATE TABLE IF NOT EXISTS dusk.lake_process_output_parquet ON CLUSTER dusk AS dusk.process_output
ENGINE = S3(lake, filename = 'process_output/dt=*/*.parquet', format = 'Parquet');

CREATE OR REPLACE VIEW dusk.lake_process_output ON CLUSTER dusk
DEFINER = default SQL SECURITY DEFINER
AS SELECT *, toDate(extract(CAST(_path AS String), '/dt=([0-9]{4}-[0-9]{2}-[0-9]{2})/')) AS dt, CAST(_path AS String) AS lake_path
FROM dusk.lake_process_output_parquet;

CREATE TABLE IF NOT EXISTS dusk.lake_files_parquet ON CLUSTER dusk AS dusk.files
ENGINE = S3(lake, filename = 'files/dt=*/*.parquet', format = 'Parquet');

CREATE OR REPLACE VIEW dusk.lake_files ON CLUSTER dusk
DEFINER = default SQL SECURITY DEFINER
AS SELECT *, toDate(extract(CAST(_path AS String), '/dt=([0-9]{4}-[0-9]{2}-[0-9]{2})/')) AS dt, CAST(_path AS String) AS lake_path
FROM dusk.lake_files_parquet;

CREATE TABLE IF NOT EXISTS dusk.lake_otel_logs_parquet ON CLUSTER dusk AS dusk.otel_logs
ENGINE = S3(lake, filename = 'otel_logs/dt=*/*.parquet', format = 'Parquet');

CREATE OR REPLACE VIEW dusk.lake_otel_logs ON CLUSTER dusk
DEFINER = default SQL SECURITY DEFINER
AS SELECT *, toDate(extract(CAST(_path AS String), '/dt=([0-9]{4}-[0-9]{2}-[0-9]{2})/')) AS dt, CAST(_path AS String) AS lake_path
FROM dusk.lake_otel_logs_parquet;

CREATE TABLE IF NOT EXISTS dusk.lake_otel_spans_parquet ON CLUSTER dusk AS dusk.otel_spans
ENGINE = S3(lake, filename = 'otel_spans/dt=*/*.parquet', format = 'Parquet');

CREATE OR REPLACE VIEW dusk.lake_otel_spans ON CLUSTER dusk
DEFINER = default SQL SECURITY DEFINER
AS SELECT *, toDate(extract(CAST(_path AS String), '/dt=([0-9]{4}-[0-9]{2}-[0-9]{2})/')) AS dt, CAST(_path AS String) AS lake_path
FROM dusk.lake_otel_spans_parquet;

CREATE TABLE IF NOT EXISTS dusk.lake_otel_metrics_parquet ON CLUSTER dusk AS dusk.otel_metrics
ENGINE = S3(lake, filename = 'otel_metrics/dt=*/*.parquet', format = 'Parquet');

CREATE OR REPLACE VIEW dusk.lake_otel_metrics ON CLUSTER dusk
DEFINER = default SQL SECURITY DEFINER
AS SELECT *, toDate(extract(CAST(_path AS String), '/dt=([0-9]{4}-[0-9]{2}-[0-9]{2})/')) AS dt, CAST(_path AS String) AS lake_path
FROM dusk.lake_otel_metrics_parquet;
