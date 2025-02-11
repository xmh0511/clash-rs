use std::{fs, io::Write, net::IpAddr, path::Path};

use async_recursion::async_recursion;
use hyper::body::HttpBody;
use maxminddb::geoip2;
use tracing::{debug, info, warn};

use crate::{
    common::{
        errors::{map_io_error, new_io_error},
        http::HttpClient,
    },
    Error,
};

pub struct MMDB {
    reader: maxminddb::Reader<Vec<u8>>,
}

impl MMDB {
    pub async fn new<P: AsRef<Path>>(
        path: P,
        download_url: Option<String>,
        http_client: HttpClient,
    ) -> Result<MMDB, Error> {
        debug!("mmdb path: {}", path.as_ref().to_string_lossy());

        let mmdb_file = path.as_ref().to_path_buf();

        if !mmdb_file.exists() {
            if let Some(url) = download_url.as_ref() {
                info!("downloading mmdb from {}", url);
                Self::download(url, &mmdb_file, &http_client)
                    .await
                    .map_err(|x| Error::InvalidConfig(format!("mmdb download failed: {}", x)))?;
            } else {
                return Err(Error::InvalidConfig(format!(
                    "mmdb `{}` not found and mmdb_download_url is not set",
                    path.as_ref().to_string_lossy()
                ))
                .into());
            }
        }

        match maxminddb::Reader::open_readfile(&path) {
            Ok(r) => Ok(MMDB { reader: r }),
            Err(e) => match e {
                maxminddb::MaxMindDBError::InvalidDatabaseError(_)
                | maxminddb::MaxMindDBError::IoError(_) => {
                    warn!(
                        "invalid mmdb `{}`: {}, trying to download again",
                        path.as_ref().to_string_lossy(),
                        e.to_string()
                    );

                    // try to download again
                    fs::remove_file(&mmdb_file)?;
                    if let Some(url) = download_url.as_ref() {
                        info!("downloading mmdb from {}", url);
                        Self::download(url, &mmdb_file, &http_client)
                            .await
                            .map_err(|x| {
                                Error::InvalidConfig(format!("mmdb download failed: {}", x))
                            })?;
                        Ok(MMDB {
                            reader: maxminddb::Reader::open_readfile(&path).map_err(|x| {
                                Error::InvalidConfig(format!(
                                    "cant open mmdb `{}`: {}",
                                    path.as_ref().to_string_lossy(),
                                    x.to_string()
                                ))
                            })?,
                        })
                    } else {
                        return Err(Error::InvalidConfig(format!(
                            "mmdb `{}` not found and mmdb_download_url is not set",
                            path.as_ref().to_string_lossy()
                        ))
                        .into());
                    }
                }
                _ => Err(Error::InvalidConfig(format!(
                    "cant open mmdb `{}`: {}",
                    path.as_ref().to_string_lossy(),
                    e.to_string()
                ))
                .into()),
            },
        }
    }

    #[async_recursion(?Send)]
    async fn download<P: AsRef<Path>>(
        url: &str,
        path: P,
        http_client: &HttpClient,
    ) -> anyhow::Result<()> {
        let uri = url.parse::<http::Uri>()?;
        let mut out = std::fs::File::create(&path)?;

        let mut res = http_client.get(uri).await?;

        if res.status().is_redirection() {
            return Self::download(
                res.headers()
                    .get("Location")
                    .ok_or(new_io_error(
                        format!("failed to download from {}", url).as_str(),
                    ))?
                    .to_str()?,
                path,
                http_client,
            )
            .await;
        }

        if !res.status().is_success() {
            return Err(
                Error::InvalidConfig(format!("mmdb download failed: {}", res.status())).into(),
            );
        }

        debug!("downloading mmdb to {}", path.as_ref().to_string_lossy());

        while let Some(chunk) = res.body_mut().data().await {
            out.write_all(&chunk?)?;
        }

        Ok(())
    }

    pub fn lookup(&self, ip: IpAddr) -> anyhow::Result<geoip2::Country> {
        self.reader
            .lookup(ip)
            .map_err(map_io_error)
            .map_err(|x| x.into())
    }
}
