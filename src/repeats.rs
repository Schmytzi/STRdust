use bio::io::bed;
use log::error;
use rust_htslib::faidx;
use std::fmt;
use std::io;

#[derive(Debug)]
pub struct RepeatIntervalIterator {
    current_index: usize,
    data: Vec<RepeatInterval>,
    pub num_intervals: usize,
}

impl RepeatIntervalIterator {
    // parse a region string
    pub fn from_string(reg: &str, fasta: &str) -> Self {
        // Split by colon first to get chromosome
        let parts: Vec<&str> = reg.split(':').collect();
        if parts.len() != 2 {
            error!("Invalid region format: '{}'. Expected format is 'chr:start-end'", reg);
            std::process::exit(1);
        }
        
        let chrom = parts[0].to_string();
        let interval = parts[1];
        
        // Split interval by hyphen to get start and end
        let coords: Vec<&str> = interval.split('-').collect();
        if coords.len() != 2 {
            error!("Invalid interval format: '{}'. Expected format is 'chr:start-end'", interval);
            error!("Example of a valid region: 'chr15:34419425-34419450'");
            std::process::exit(1);
        }
        
        // Parse start and end coordinates with error handling
        let start: u32 = match coords[0].parse() {
            Ok(val) => val,
            Err(_) => {
                error!("Could not parse start coordinate '{}' as a number", coords[0]);
                std::process::exit(1);
            }
        };
        
        let end: u32 = match coords[1].parse() {
            Ok(val) => val,
            Err(_) => {
                error!("Could not parse end coordinate '{}' as a number", coords[1]);
                std::process::exit(1);
            }
        };
        
        // Create the repeat interval with validation
        let repeat = match RepeatInterval::new_interval(chrom, start, end, fasta) {
            Some(interval) => interval,
            None => {
                error!("Failed to create repeat interval for region: '{}'", reg);
                std::process::exit(1);
            }
        };
        
        RepeatIntervalIterator {
            current_index: 0,
            data: vec![repeat],
            num_intervals: 1,
        }
    }

    pub fn from_bed(region_file: &String, fasta: &str) -> Self {
        // check if the bed file exists
        if !std::path::Path::new(region_file).exists() {
            error!("Bed file does not exist");
            std::process::exit(1);
        }
        let mut reader = bed::Reader::from_file(region_file).expect("Problem reading bed file!");
        let mut data = Vec::new();
        for record in reader.records() {
            let rec = record.expect("Error reading bed record. Please verify bed file is in the correct format and without header.");
            let repeat = RepeatInterval::from_bed(&rec, fasta);
            if let Some(repeat) = repeat {
                data.push(repeat);
            }
        }
        RepeatIntervalIterator {
            current_index: 0,
            data: data.clone(),
            num_intervals: data.len(),
        }
    }

    pub fn pathogenic(fasta: &str) -> Self {
        let url = "https://github.com/dashnowlab/STRchive/raw/refs/heads/main/data/STRchive-disease-loci.hg38.TRGT.bed";
        let resp = reqwest::blocking::get(url).expect("request to STRchive failed");
        let body = resp.text().expect("body invalid");
        let mut reader = io::BufReader::new(body.as_bytes());
        let mut data = Vec::new();
        let mut reader = bed::Reader::new(&mut reader);
        for record in reader.records() {
            let rec = record.expect("Error reading bed record.");
            let repeat = RepeatInterval::from_bed(&rec, fasta);
            if let Some(repeat) = repeat {
                data.push(repeat);
            }
        }
        RepeatIntervalIterator {
            current_index: 0,
            data: data.clone(),
            num_intervals: data.len(),
        }
    }
}

impl Clone for RepeatInterval {
    fn clone(&self) -> Self {
        RepeatInterval {
            chrom: self.chrom.clone(),
            start: self.start,
            end: self.end,
            created: self.created,
        }
    }
}

impl Iterator for RepeatIntervalIterator {
    type Item = RepeatInterval;

    fn next(&mut self) -> Option<Self::Item> {
        // Implement the logic to get the next RepeatInterval here.
        // This is a simple example that gets the next item from a vector.
        if self.current_index < self.data.len() {
            let result = self.data[self.current_index].clone();
            self.current_index += 1;
            Some(result)
        } else {
            None
        }
    }
}

#[derive(Debug)]
pub struct RepeatInterval {
    pub chrom: String,
    pub start: u32,
    pub end: u32,
    pub created: Option<chrono::DateTime<chrono::Utc>>,
}

impl fmt::Display for RepeatInterval {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:{}-{}", self.chrom, self.start, self.end)
    }
}

impl RepeatInterval {
    /// Make a repeat compressed sequence from a fasta file
    /// The repeat sequence is removed, and <flanking> bases of flanking sequence is added on either side
    /// If the extended interval (+/- flanking) is out of bounds, the extended interval is shortened automatically by faidx

    // parse a bed record
    pub fn from_bed(rec: &bed::Record, fasta: &str) -> Option<Self> {
        let chrom = rec.chrom().to_string();
        let start = rec.start().try_into().unwrap();
        let end = rec.end().try_into().unwrap();
        RepeatInterval::new_interval(chrom, start, end, fasta)
    }

    fn new_interval(chrom: String, start: u32, end: u32, fasta: &str) -> Option<Self> {
        if end < start {
            panic!("End coordinate is smaller than start coordinate for {chrom}:{start}-{end}")
        }

        let fai = format!("{}.fai", fasta);
        // check if the chromosome exists in the fai file
        // and if the end coordinate is within the chromosome length
        for line in std::fs::read_to_string(fai)
            .expect("Failed to read fai file")
            .lines()
        {
            if line.split('\t').collect::<Vec<&str>>()[0] == chrom
                && line.split('\t').collect::<Vec<&str>>()[1]
                    .parse::<u32>()
                    .expect("Failed parsing chromosome length from fai file")
                    > end
            {
                return Some(Self { chrom, start, end, created: None });
            }
        }
        // if the chromosome is not in the fai file or the end does not fit the interval, return None
        panic!(
            "Chromosome {chrom} is not in the fasta file or the end coordinate is out of bounds"
        );
    }
    pub fn new(chrom: &str, start: u32, end: u32) -> Self {
        Self {
            chrom: chrom.to_string(),
            start,
            end,
            created: None
        }
    }

    pub fn make_repeat_compressed_sequence(&self, fasta: &String, flanking: u32) -> Vec<u8> {
        let fas = faidx::Reader::from_path(fasta).expect("Failed to read fasta");
        let fas_left = fas
            .fetch_seq(
                &self.chrom,
                (self.start.saturating_sub(flanking + 2)) as usize,
                self.start as usize - 2,
            )
            .expect("Failed to extract fas_left sequence from fasta for {chrom}:{start}-{end}");
        let fas_right = fas
            .fetch_seq(
                &self.chrom,
                self.end as usize,
                (self.end + flanking - 2) as usize,
            )
            .expect("Failed to extract fas_right sequence from fasta for {chrom}:{start}-{end}");
        [fas_left, fas_right].concat()
    }

    pub fn reference_repeat_sequence(&self, fasta: &String) -> Option<String> {
        let fas = faidx::Reader::from_path(fasta).expect("Failed to read fasta");
        let repeat_ref_sequence = std::str::from_utf8(
            &fas.fetch_seq(&self.chrom, self.start as usize - 1, self.end as usize)
                .expect("Failed to extract repeat sequence from fasta for {chrom}:{start}-{end}"),
        )
        .expect("Failed to convert repeat sequence to string for {chrom}:{start}-{end}")
        .to_string();

        // If the repeat sequence is out of bounds, None is returned
        if repeat_ref_sequence == "N" {
            eprintln!(
                "Cannot genotype repeat at {self} because it is out of bounds for the fasta file",
            );
            return None;
        }
        Some(repeat_ref_sequence)
    }

    pub fn set_time_stamp(& mut self) {
        self.created = Some(chrono::Utc::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_make_repeat_compressed_sequence() {
        let fasta = String::from("test_data/chr7.fa.gz");
        let repeat = RepeatIntervalIterator::from_string("chr7:154654404-154654432", &fasta);
        let flanking = 2000;
        for r in repeat {
            let newref = r.make_repeat_compressed_sequence(&fasta, flanking);
            assert_eq!(newref.len() as u32, flanking * 2);
        }
    }

    // captures the case when the repeat interval is out of bounds for the fasta file
    #[test]
    #[should_panic]
    fn test_interval_out_of_bounds() {
        let fasta = String::from("test_data/chr7.fa.gz");
        let _ = RepeatIntervalIterator::from_string("chr7:1154654404-1154654432", &fasta);
    }

    // capture the case where the newref extended by <flanking> bases is out of bounds (<0) for the fasta file
    // and thus a shorter sequence is returned
    #[test]
    fn test_make_newref_interval_out_of_bounds1() {
        let fasta = String::from("test_data/chr7.fa.gz");
        let repeat = RepeatIntervalIterator::from_string("chr7:1000-5010", &fasta);
        let flanking = 2000;
        for r in repeat {
            let newref = r.make_repeat_compressed_sequence(&fasta, flanking);
            assert!((newref.len() as u32) < flanking * 2);
        }
    }

    // capture the case where the newref extended by <flanking>b is out of bounds (> chromosome end) for the fasta file
    #[test]
    fn test_make_newref_interval_out_of_bounds2() {
        let fasta = String::from("test_data/chr7.fa.gz");
        let repeat = RepeatIntervalIterator::from_string("chr7:159345373-159345400", &fasta);
        let flanking = 2000;
        for r in repeat {
            let newref = r.make_repeat_compressed_sequence(&fasta, flanking);
            assert!((newref.len() as u32) < flanking * 2);
        }
    }

    #[test]
    #[should_panic]
    fn test_make_newref_interval_invalid_chrom() {
        let fasta = String::from("test_data/chr7.fa.gz");
        let _ = crate::repeats::RepeatIntervalIterator::from_string(
            "chr27:159345373-159345400",
            &fasta,
        );
    }

    // this test is ignored as it uses a file outside the test_data directory
    #[test]
    #[ignore]
    fn test_pathogenic() {
        let fasta = String::from("/home/wdecoster/reference/GRCh38.fa");
        let _ = crate::repeats::RepeatIntervalIterator::pathogenic(&fasta);
    }
}
