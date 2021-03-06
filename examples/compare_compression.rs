//! Compare compression results

extern crate binjs;
extern crate clap;
extern crate env_logger;
extern crate glob;
extern crate rand;

use binjs::io::bytes::compress::*;
use binjs::io::TokenSerializer;
use binjs::generic::FromJSON;
use binjs::source::*;

use clap::*;

use std::collections::HashMap;
use std::io::Write;
use std::process::Command;

#[derive(Clone)]
struct Sizes {
    uncompressed: usize,
    gzip: usize,
    bzip2: usize,
    brotli: usize,
}
/*
impl std::ops::Add for Sizes {
    type Output = Self;
    fn add(self, rhs: Sizes) -> Sizes {
        self.uncompressed += rhs.uncompressed;
        self.gzip += rhs.gzip;
        self.bzip2 += rhs.bzip2;
        self.brotli += rhs.brotli;
        self
    }
}
*/
#[derive(Clone)]
struct FileStats {
    from_text: Sizes,
    from_binjs: Sizes,
    binjs_stats: binjs::io::multipart::Statistics,
}

fn get_compressed_sizes(path: &std::path::Path) -> Sizes {
    let uncompressed = std::fs::metadata(path)
        .expect("Could not open source")
        .len() as usize;
    let gzip = {
        let out = Command::new("gzip")
            .arg("--keep")
            .arg("--best")
            .arg("--stdout")
            .arg(path)
            .output()
            .expect("Error during gzip");
        assert!(out.status.success());
        assert!(out.stdout.len() != 0);
        out.stdout.len()
    };
    let bzip2 = {
        let out = Command::new("bzip2")
            .arg("--keep")
            .arg("--best")
            .arg("--stdout")
            .arg(path)
            .output()
            .expect("Error during bzip2");
        assert!(out.status.success());
        assert!(out.stdout.len() != 0);
        out.stdout.len()
    };
    let brotli = {
        let out = Command::new("brotli")
            .arg("--best")
            .arg("--keep")
            .arg("--stdout")
            .arg(path)
            .output()
            .expect("Error during brotli");
        assert!(out.status.success());
        assert!(out.stdout.len() != 0);
        out.stdout.len()
    };
    Sizes {
        bzip2,
        brotli,
        gzip,
        uncompressed,
    }
}

fn main() {
    env_logger::init();
    let dest_path_binjs = "/tmp/binjs-test.js.binjs";

    let matches = App::new("Compare BinJS compression and brotli/gzip compression")
        .author("David Teller <dteller@mozilla.com>")
        .args(&[
            Arg::with_name("in")
                .long("in")
                .short("i")
                .multiple(true)
                .required(true)
                .takes_value(true)
                .help("Glob path towards source files"),
            Arg::with_name("compression")
                .long("compression")
                .short("c")
                .required(true)
                .takes_value(true)
                .possible_values(&["identity", "gzip", "br", "deflate"])
                .help("Compression format for the binjs files"),
        ])
        .get_matches();

    let compression = matches.value_of("compression")
        .expect("Missing compression format");
    let compression = Compression::parse(Some(compression))
        .expect("Could not parse compression format");
    let binjs_options = {
        binjs::io::multipart::WriteOptions {
            strings_table: compression.clone(),
            grammar_table: compression.clone(),
            tree: compression.clone()
        }
    };

    let parser = Shift::new();

    let mut multipart_stats = binjs::io::multipart::Statistics::default()
        .with_source_bytes(0);

    let mut all_stats = HashMap::new();

    for path in matches.values_of("in").expect("Missing `in`") {
        for source_path in glob::glob(&path).expect("Invalid pattern") {
            let source_path = source_path.expect("I/O error");
            eprintln!("Source: {}", source_path.to_str().expect("Could not display path"));

            let from_text = get_compressed_sizes(&source_path);

            eprintln!("Compressing with binjs");
            let json = parser.parse_file(source_path.clone())
                .expect("Could not parse source");
            let mut ast = binjs::specialized::es6::ast::Script::import(&json)
                .expect("Could not import AST");
            binjs::specialized::es6::scopes::AnnotationVisitor::new()
                .annotate_script(&mut ast);

            let writer = binjs::io::multipart::TreeTokenWriter::new(binjs_options.clone());
            let mut serializer = binjs::specialized::es6::io::Serializer::new(writer);
            serializer.serialize(&ast)
                .expect("Could not encode AST");
            let (data, stats) = serializer.done()
                .expect("Could not finalize AST encoding");

            let binjs_stats = stats.clone();
            multipart_stats = multipart_stats + stats.with_source_bytes(from_text.uncompressed as usize);

            {
                let mut binjs_encoded = std::fs::File::create(&dest_path_binjs)
                    .expect("Could not create binjs-encoded file");
                binjs_encoded.write_all(&data)
                    .expect("Could not write binjs-encoded file");
            }

            let from_binjs = get_compressed_sizes(&std::path::Path::new(dest_path_binjs));

            let file_stats = FileStats {
                from_binjs,
                from_text,
                binjs_stats,
            };

            eprintln!("Compression results: source {source}b, source+gzip {source_gzip}, source+brotli {source_brotli}, source+bzip2 {source_bzip2}, binjs {binjs}b, binjs+gzip {binjs_gzip}, binjs+brotli {binjs_brotli}, binjs+bzip2 {binjs_bzip2}",
                source = file_stats.from_text.uncompressed,
                source_gzip = file_stats.from_text.gzip,
                source_brotli = file_stats.from_text.brotli,
                source_bzip2 = file_stats.from_text.bzip2,

                binjs = file_stats.from_binjs.uncompressed,
                binjs_gzip = file_stats.from_binjs.gzip,
                binjs_brotli = file_stats.from_binjs.brotli,
                binjs_bzip2 = file_stats.from_binjs.bzip2,
            );

            all_stats.insert(source_path, file_stats);
        }
    }

    eprintln!("*** Done");
    eprintln!("File, Source, Source+Gzip, Source+Brotli, Source+BZip2, Binjs, Binjs+GZip, Binjs+Brotli, Binjs+BZip2, Number of strings, Number of identifiers, Number of grammar entries");
    for (path, file_stats) in &all_stats {
        let number_of_binding_identifiers = match file_stats.binjs_stats.per_kind_name.get("BindingIdentifier") {
            None => 0,
            Some(identifiers) => identifiers.entries
        };
        let number_of_expression_identifiers = match file_stats.binjs_stats.per_kind_name.get("IdentifierExpression") {
            None => 0,
            Some(identifiers) => identifiers.entries
        };

        println!("{path:?}, {source}, {source_gzip}, {source_brotli}, {source_bzip2}, {binjs}, {binjs_gzip}, {binjs_brotli}, {binjs_bzip2}, {strings}, {identifiers}, {grammar_entries}",
            source = file_stats.from_text.uncompressed,
            source_gzip = file_stats.from_text.gzip,
            source_brotli = file_stats.from_text.brotli,
            source_bzip2 = file_stats.from_text.bzip2,

            binjs = file_stats.from_binjs.uncompressed,
            binjs_gzip = file_stats.from_binjs.gzip,
            binjs_brotli = file_stats.from_binjs.brotli,
            binjs_bzip2 = file_stats.from_binjs.bzip2,

            strings = file_stats.binjs_stats.strings_table.entries,
            identifiers = number_of_binding_identifiers + number_of_expression_identifiers,
            grammar_entries = file_stats.binjs_stats.grammar_table.entries,
            path = path);
    }
}

