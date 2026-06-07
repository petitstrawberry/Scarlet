use std::collections::BTreeMap;
use std::error::Error;
use std::io::{self, Write};

const SAMPLE: &str = "\
name,category,count
kernel,core,3
shell,user,5
desktop,user,2
driver,core,7
";

#[derive(Debug)]
struct Row {
    name: String,
    category: String,
    count: u32,
}

fn main() -> Result<(), Box<dyn Error>> {
    let rows = read_rows(SAMPLE.as_bytes())?;
    let totals = totals_by_category(&rows);
    let report = write_report(&rows, &totals)?;

    let mut stdout = io::stdout();
    stdout.write_all(report.as_bytes())?;
    stdout.flush()?;

    Ok(())
}

fn read_rows(input: &[u8]) -> Result<Vec<Row>, Box<dyn Error>> {
    let mut reader = csv::Reader::from_reader(input);
    let mut rows = Vec::new();

    for record in reader.records() {
        let record = record?;
        let name = record.get(0).ok_or("missing name")?.to_owned();
        let category = record.get(1).ok_or("missing category")?.to_owned();
        let count = record.get(2).ok_or("missing count")?.parse::<u32>()?;

        rows.push(Row {
            name,
            category,
            count,
        });
    }

    Ok(rows)
}

fn totals_by_category(rows: &[Row]) -> BTreeMap<String, u32> {
    let mut totals = BTreeMap::new();
    for row in rows {
        *totals.entry(row.category.clone()).or_insert(0) += row.count;
    }
    totals
}

fn write_report(rows: &[Row], totals: &BTreeMap<String, u32>) -> Result<String, Box<dyn Error>> {
    let mut output = Vec::new();
    {
        let mut writer = csv::Writer::from_writer(&mut output);
        writer.write_record(["kind", "name", "category", "count"])?;

        for row in rows {
            writer.write_record([
                "row",
                row.name.as_str(),
                row.category.as_str(),
                &row.count.to_string(),
            ])?;
        }

        for (category, count) in totals {
            writer.write_record([
                "total",
                category.as_str(),
                category.as_str(),
                &count.to_string(),
            ])?;
        }

        writer.flush()?;
    }

    Ok(String::from_utf8(output)?)
}
