// Copyright (c) 2018 10x Genomics, Inc. All rights reserved.

//! Utility methods.

use std::fs;
use std::mem;
use std::fs::File;
use std::io::Write;
use std::hash::Hash;
use std::fmt::Debug;
use std::boxed::Box;
use std::path::{Path};
use std::io::{BufRead, BufReader, BufWriter};

//use bincode;
use bincode;
use boomphf;
use debruijn::{Kmer, Vmer};
use serde::{Serialize};
use serde::de::DeserializeOwned;
use debruijn::graph::DebruijnGraph;
use debruijn::filter::EqClassIdType;
use bincode::{serialize_into, deserialize_from};

use failure::Error;
use flate2::read::MultiGzDecoder;

use std::marker::PhantomData;
use config::MAX_WORKER;

#[derive(Serialize, Deserialize, Debug)]
pub struct Index<K, D>
where K:Hash + Serialize, D: Eq + Hash + Serialize {
    eqclasses: Vec<Vec<D>>,
    dbg: DebruijnGraph<K, EqClassIdType>,
    phf: boomphf::NoKeyBoomHashMap2<K, usize, u32>,
}

impl<K, D> Index<K, D>
where K:Hash + Serialize + Kmer + Send + Sync + DeserializeOwned + Send + Sync,
      D: Clone + Debug + Eq + Hash + Serialize + DeserializeOwned{
    pub fn dump(dbg: DebruijnGraph<K, EqClassIdType>,
                gene_order: Vec<String>,
                eqclasses: Vec<Vec<D>>,
                index_path: &str) {

        info!("Dumping index into folder: {:?}", index_path);
        match fs::create_dir(index_path) {
            Err(err) => warn!("{:?}", err),
            Ok(()) => info!("Creating folder {:?}", index_path),
        }

        let data_type: usize = mem::size_of::<D>();
        write_obj(&data_type, index_path.to_owned() + "/type.bin").expect("Can't dump data type");

        let eqclass_file_name = index_path.to_owned() + "/eq_classes.bin";
        write_obj(&eqclasses, eqclass_file_name).expect("Can't dump classes");

        info!("Found {} Equivalence classes", eqclasses.len());

        let genes_file_name = index_path.to_owned() + "/genes.txt";
        let mut file_handle = File::create(genes_file_name).expect("Unable to create file");
        for i in gene_order{
            write!(file_handle, "{}\n", i).expect("can't write gene names");
        }

        let dbg_file_name = index_path.to_owned() + "/dbg.bin";
        write_obj(&dbg, dbg_file_name).expect("Can't dump debruijn graph");

        let mut total_kmers = 0;
        let kmer_length = K::k();
        for node in dbg.iter_nodes() {
            total_kmers += node.len()-kmer_length+1;
        }
        println!("Total {:?} kmers to process in dbg", total_kmers);

        let mut node_ids: Vec<usize> = vec![0; total_kmers];
        let mut offsets: Vec<u32> = vec![0; total_kmers];

        let mphf = boomphf::Mphf::new_parallel_with_key(1.7, &dbg, None,
                                                        total_kmers,
                                                        MAX_WORKER);
        for node in dbg.iter_nodes() {
            let mut offset = 0;
            for kmer in node.sequence().iter_kmers::<K>() {
                let index = match mphf.try_hash(&kmer){
                    Some(index) => index,
                    None => panic!("Nothing found for {:?}", kmer),
                };
                node_ids[index as usize] = node.node_id;
                offsets[index as usize] = offset;
                offset += 1;
            }
        }

        let phf = boomphf::NoKeyBoomHashMap2 {
            mphf: mphf,
            phantom: PhantomData,
            values: node_ids,
            aux_values: offsets,
        };

        let phf_file_name = index_path.to_owned() + "/phf.bin";
        write_obj(&phf, phf_file_name).expect("Can't dump phf");
    }

    pub fn read(index_path: &str) -> Index<K, D> {

        match fs::read_dir(index_path) {
            Err(_) => panic!("{:?} directory not found", index_path),
            Ok(_) => info!("Reading index from folder: {:?}", index_path),
        }

        let eqclass_file_name = index_path.to_owned() + "/eq_classes.bin";
        let eq_classes: Vec<Vec<D>> = read_obj(eqclass_file_name)
            .expect("Can't read classes");

        let dbg_file_name = index_path.to_owned() + "/dbg.bin";
        let dbg = read_obj(dbg_file_name).expect("Can't read debruijn graph");

        let phf_file_name = index_path.to_owned() + "/phf.bin";
        let phf = read_obj(phf_file_name).expect("Can't read phf");

        Index{
            eqclasses: eq_classes,
            dbg: dbg,
            phf: phf,
        }
    }

    pub fn get_phf(&self) -> &boomphf::NoKeyBoomHashMap2<K, usize, u32>{
        &self.phf
    }

    pub fn get_dbg(&self) -> &DebruijnGraph<K, EqClassIdType>{
        &self.dbg
    }

    pub fn get_eq_classes(&self) -> &Vec<Vec<D>>{
        &self.eqclasses
    }
}

pub fn get_data_type(index_path: &str) -> usize {
    match fs::read_dir(index_path) {
        Err(_) => panic!("{:?} directory not found", index_path),
        Ok(_) => info!("Reading index from folder: {:?}", index_path),
    }

    let data_type_file_name = index_path.to_owned() + "/type.bin";
    let data_type: usize = read_obj(data_type_file_name).expect("Can't read data type");
    data_type
}

/// Open a (possibly gzipped) file into a BufReader.
fn _open_with_gz<P: AsRef<Path>>(p: P) -> Result<Box<BufRead>, Error> {
    let r = File::open(p.as_ref())?;

    if p.as_ref().extension().unwrap() == "gz" {
        let gz = MultiGzDecoder::new(r);
        let buf_reader = BufReader::with_capacity(32*1024, gz);
        Ok(Box::new(buf_reader))
    } else {
        let buf_reader = BufReader::with_capacity(32*1024, r);
        Ok(Box::new(buf_reader))
    }
}



fn write_obj<T: Serialize, P: AsRef<Path> + Debug>(g: &T, filename: P) -> Result<(), bincode::Error> {
    let f = match File::create(&filename) {
        Err(err) => panic!("couldn't create file {:?}: {}", filename, err),
        Ok(f) => f,
    };
    let mut writer = BufWriter::new(f);
    serialize_into(&mut writer, &g)
}

pub fn read_obj<T: DeserializeOwned, P: AsRef<Path> + Debug>(filename: P) -> Result<T, bincode::Error> {
    let f = match File::open(&filename) {
        Err(err) => panic!("couldn't open file {:?}: {}", filename, err),
        Ok(f) => f,
    };
    let mut reader = BufReader::new(f);
    deserialize_from(&mut reader)
}
