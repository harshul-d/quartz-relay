//! Writes the two output tables as Parquet (rules.md "Outputs").

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use parquet::basic::Compression;
use parquet::data_type::{ByteArray, ByteArrayType, Int32Type, Int64Type};
use parquet::file::properties::{EnabledStatistics, WriterProperties};
use parquet::file::writer::{SerializedFileWriter, SerializedRowGroupWriter};
use parquet::schema::parser::parse_message_type;

use crate::key::Strings;
use crate::{Lookups, NO_CATEGORY, ThreadResult, UserDay, Window, par};

const USER_DAILY: &str = "
message user_daily {
  REQUIRED BYTE_ARRAY user_id (UTF8);
  REQUIRED INT32 day (DATE);
  REQUIRED INT64 sessions;
  REQUIRED INT64 events;
  REQUIRED INT64 revenue_cents;
  OPTIONAL BYTE_ARRAY top_category (UTF8);
}";

const CAMPAIGN_DAILY: &str = "
message campaign_daily {
  REQUIRED BYTE_ARRAY campaign_id (UTF8);
  REQUIRED BYTE_ARRAY channel (UTF8);
  REQUIRED INT32 day (DATE);
  REQUIRED INT64 sessions;
  REQUIRED INT64 purchasing_sessions;
  REQUIRED INT64 revenue_cents;
  REQUIRED INT64 assisted_sessions;
}";

fn properties() -> Arc<WriterProperties> {
    Arc::new(
        WriterProperties::builder()
            .set_compression(Compression::UNCOMPRESSED)
            .set_dictionary_enabled(false)
            .set_statistics_enabled(EnabledStatistics::None)
            .build(),
    )
}

fn writer(path: &Path, schema: &str) -> SerializedFileWriter<File> {
    let schema = Arc::new(parse_message_type(schema).expect("schema"));
    let file = File::create(path).unwrap_or_else(|error| panic!("create {}: {error}", path.display()));
    SerializedFileWriter::new(file, schema, properties()).expect("parquet writer")
}

fn bytes_column(group: &mut SerializedRowGroupWriter<File>, values: &[ByteArray], levels: Option<&[i16]>) {
    let mut column = group.next_column().expect("column").expect("column exists");
    column.typed::<ByteArrayType>().write_batch(values, levels, None).expect("write");
    column.close().expect("close column");
}

fn int32_column(group: &mut SerializedRowGroupWriter<File>, values: &[i32]) {
    let mut column = group.next_column().expect("column").expect("column exists");
    column.typed::<Int32Type>().write_batch(values, None, None).expect("write");
    column.close().expect("close column");
}

fn int64_column(group: &mut SerializedRowGroupWriter<File>, values: &[i64]) {
    let mut column = group.next_column().expect("column").expect("column exists");
    column.typed::<Int64Type>().write_batch(values, None, None).expect("write");
    column.close().expect("close column");
}

fn write_user_daily(path: &Path, rows: &[UserDay], lookups: &Lookups, strings: &Strings) {
    let mut file = writer(path, USER_DAILY);
    if !rows.is_empty() {
        let mut group = file.next_row_group().expect("row group");
        let ids: Vec<ByteArray> = rows.iter().map(|row| ByteArray::from(strings.bytes(row.person))).collect();
        bytes_column(&mut group, &ids, None);
        drop(ids);
        int32_column(&mut group, &rows.iter().map(|row| row.day).collect::<Vec<_>>());
        int64_column(&mut group, &rows.iter().map(|row| row.sessions as i64).collect::<Vec<_>>());
        int64_column(&mut group, &rows.iter().map(|row| row.events as i64).collect::<Vec<_>>());
        int64_column(&mut group, &rows.iter().map(|row| row.revenue as i64).collect::<Vec<_>>());
        let levels: Vec<i16> = rows.iter().map(|row| (row.top != NO_CATEGORY) as i16).collect();
        let tops: Vec<ByteArray> = rows
            .iter()
            .filter(|row| row.top != NO_CATEGORY)
            .map(|row| ByteArray::from(lookups.categories[row.top as usize].clone()))
            .collect();
        bytes_column(&mut group, &tops, Some(&levels));
        group.close().expect("close row group");
    }
    file.close().expect("close parquet");
}

pub fn write_all(output: &Path, results: Vec<ThreadResult>, lookups: &Lookups, window: &Window, strings: &Strings, threads: usize) {
    let user_dir = output.join("user_daily");
    let campaign_dir = output.join("campaign_daily");
    std::fs::create_dir_all(&user_dir).expect("create user_daily");
    std::fs::create_dir_all(&campaign_dir).expect("create campaign_daily");

    // campaign_daily: sum the per-thread counts.
    let days = window.days;
    let mut totals = vec![[0u64; 4]; (lookups.campaign_names.len() + 1) * days];
    for result in &results {
        for (total, counts) in totals.iter_mut().zip(&result.campaigns) {
            for k in 0..4 {
                total[k] += counts[k];
            }
        }
    }
    let mut campaign_ids = Vec::new();
    let mut channels = Vec::new();
    let mut day_values = Vec::new();
    let mut columns: [Vec<i64>; 4] = Default::default();
    for (index, counts) in totals.iter().enumerate() {
        if counts[0] == 0 && counts[3] == 0 {
            continue;
        }
        let campaign = index / days.max(1);
        let (name, channel): (&[u8], &[u8]) = if campaign == lookups.campaign_names.len() {
            (b"organic", b"organic")
        } else {
            (&lookups.campaign_names[campaign], &lookups.channels[campaign])
        };
        campaign_ids.push(ByteArray::from(name.to_vec()));
        channels.push(ByteArray::from(channel.to_vec()));
        day_values.push((window.first_day + (index % days) as i64) as i32);
        for k in 0..4 {
            columns[k].push(counts[k] as i64);
        }
    }

    let user_rows: Vec<Vec<UserDay>> = results.into_iter().map(|result| result.user_days).collect();
    let jobs = user_rows.len().max(1);
    par::run(threads, jobs + 1, |_| (), |_, job| {
        if job < jobs {
            let rows: &[UserDay] = user_rows.get(job).map_or(&[], |rows| rows.as_slice());
            if rows.is_empty() && job != 0 {
                return;
            }
            write_user_daily(&user_dir.join(format!("part-{job:03}.parquet")), rows, lookups, strings);
        } else {
            let mut file = writer(&campaign_dir.join("part-000.parquet"), CAMPAIGN_DAILY);
            if !campaign_ids.is_empty() {
                let mut group = file.next_row_group().expect("row group");
                bytes_column(&mut group, &campaign_ids, None);
                bytes_column(&mut group, &channels, None);
                int32_column(&mut group, &day_values);
                for column in &columns {
                    int64_column(&mut group, column);
                }
                group.close().expect("close row group");
            }
            file.close().expect("close parquet");
        }
    });
}
