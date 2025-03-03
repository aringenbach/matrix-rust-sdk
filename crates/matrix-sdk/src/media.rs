// Copyright 2021 Kévin Commaille
// Copyright 2022 The Matrix.org Foundation C.I.C.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! High-level media API.

#[cfg(feature = "e2e-encryption")]
use std::io::Read;
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use std::time::Duration;

use eyeball::shared::Observable as SharedObservable;
pub use matrix_sdk_base::media::*;
use mime::Mime;
#[cfg(not(target_arch = "wasm32"))]
use mime2ext;
use ruma::{
    api::client::media::{create_content, get_content, get_content_thumbnail},
    assign,
    events::room::MediaSource,
    MxcUri,
};
#[cfg(not(target_arch = "wasm32"))]
use tempfile::{Builder as TempFileBuilder, NamedTempFile, TempDir};
#[cfg(not(target_arch = "wasm32"))]
use tokio::{fs::File as TokioFile, io::AsyncWriteExt};

use crate::{
    attachment::{AttachmentInfo, Thumbnail},
    Client, Result, SendRequest, TransmissionProgress,
};

/// A conservative upload speed of 1Mbps
const DEFAULT_UPLOAD_SPEED: u64 = 125_000;
/// 5 min minimal upload request timeout, used to clamp the request timeout.
const MIN_UPLOAD_REQUEST_TIMEOUT: Duration = Duration::from_secs(60 * 5);

/// A high-level API to interact with the media API.
#[derive(Debug, Clone)]
pub struct Media {
    /// The underlying HTTP client.
    client: Client,
}

/// A file handle that takes ownership of a media file on disk. When the handle
/// is dropped, the file will be removed from the disk.
#[derive(Debug)]
#[cfg(not(target_arch = "wasm32"))]
pub struct MediaFileHandle {
    /// The temporary file that contains the media.
    file: NamedTempFile,
    /// An intermediary temporary directory used in certain cases.
    ///
    /// Only stored for its `Drop` semantics.
    _directory: Option<TempDir>,
}

#[cfg(not(target_arch = "wasm32"))]
impl MediaFileHandle {
    /// Get the media file's path.
    pub fn path(&self) -> &Path {
        self.file.path()
    }
}

/// `IntoFuture` returned by [`Media::upload`].
pub type SendUploadRequest = SendRequest<create_content::v3::Request>;

impl Media {
    pub(crate) fn new(client: Client) -> Self {
        Self { client }
    }

    /// Upload some media to the server.
    ///
    /// # Arguments
    ///
    /// * `content_type` - The type of the media, this will be used as the
    /// content-type header.
    ///
    /// * `reader` - A `Reader` that will be used to fetch the raw bytes of the
    /// media.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::fs;
    /// # use matrix_sdk::{Client, ruma::room_id};
    /// # use url::Url;
    /// # use mime;
    /// # async {
    /// # let homeserver = Url::parse("http://localhost:8080")?;
    /// # let mut client = Client::new(homeserver).await?;
    /// let image = fs::read("/home/example/my-cat.jpg")?;
    ///
    /// let response = client.media().upload(&mime::IMAGE_JPEG, image).await?;
    ///
    /// println!("Cat URI: {}", response.content_uri);
    /// # anyhow::Ok(()) };
    /// ```
    pub fn upload(&self, content_type: &Mime, data: Vec<u8>) -> SendUploadRequest {
        let timeout = std::cmp::max(
            Duration::from_secs(data.len() as u64 / DEFAULT_UPLOAD_SPEED),
            MIN_UPLOAD_REQUEST_TIMEOUT,
        );

        let request = assign!(create_content::v3::Request::new(data), {
            content_type: Some(content_type.essence_str().to_owned()),
        });

        let request_config = self.client.request_config().timeout(timeout);
        self.client.send(request, Some(request_config))
    }

    /// Gets a media file by copying it to a temporary location on disk.
    ///
    /// The file won't be encrypted even if it is encrypted on the server.
    ///
    /// Returns a `MediaFileHandle` which takes ownership of the file. When the
    /// handle is dropped, the file will be deleted from the temporary location.
    ///
    /// # Arguments
    ///
    /// * `request` - The `MediaRequest` of the content.
    ///
    /// * `content_type` - The type of the media, this will be used to set the
    ///   temporary file's extension.
    ///
    /// * `use_cache` - If we should use the media cache for this request.
    ///
    /// * `temp_dir` - Path to a directory where temporary directories can be
    ///   created. If not provided, a default, global temporary directory will
    ///   be used; this may not work properly on Android, where the default
    ///   location may require root access on some older Android versions.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn get_media_file(
        &self,
        request: &MediaRequest,
        body: Option<String>,
        content_type: &Mime,
        use_cache: bool,
        temp_dir: Option<String>,
    ) -> Result<MediaFileHandle> {
        let data = self.get_media_content(request, use_cache).await?;

        let inferred_extension = mime2ext::mime2ext(content_type);

        let body_path = body.as_ref().map(Path::new);
        let filename = body_path.and_then(|f| f.file_name().and_then(|f| f.to_str()));
        let filename_with_extension = body_path.and_then(|f| {
            if f.extension().is_some() {
                f.file_name().and_then(|f| f.to_str())
            } else {
                None
            }
        });

        let (temp_file, temp_dir) = match (filename, filename_with_extension, inferred_extension) {
            // If the body is a file name and has an extension use that
            (Some(_), Some(filename_with_extension), Some(_)) => {
                // Use an intermediary directory to avoid conflicts
                let temp_dir = temp_dir.map(TempDir::new_in).unwrap_or_else(TempDir::new)?;
                let temp_file = TempFileBuilder::new()
                    .prefix(filename_with_extension)
                    .rand_bytes(0)
                    .tempfile_in(&temp_dir)?;
                (temp_file, Some(temp_dir))
            }
            // If the body is a file name but doesn't have an extension try inferring one for it
            (Some(filename), None, Some(inferred_extension)) => {
                // Use an intermediary directory to avoid conflicts
                let temp_dir = temp_dir.map(TempDir::new_in).unwrap_or_else(TempDir::new)?;
                let temp_file = TempFileBuilder::new()
                    .prefix(filename)
                    .suffix(&(".".to_owned() + inferred_extension))
                    .rand_bytes(0)
                    .tempfile_in(&temp_dir)?;
                (temp_file, Some(temp_dir))
            }
            // If the only thing we have is an inferred extension then use that together with a
            // randomly generated file name
            (None, None, Some(inferred_extension)) => (
                TempFileBuilder::new()
                    .suffix(&&(".".to_owned() + inferred_extension))
                    .tempfile()?,
                None,
            ),
            // Otherwise just use a completely random file name
            _ => (TempFileBuilder::new().tempfile()?, None),
        };

        TokioFile::from_std(temp_file.reopen()?).write_all(&data).await?;

        Ok(MediaFileHandle { file: temp_file, _directory: temp_dir })
    }

    /// Get a media file's content.
    ///
    /// If the content is encrypted and encryption is enabled, the content will
    /// be decrypted.
    ///
    /// # Arguments
    ///
    /// * `request` - The `MediaRequest` of the content.
    ///
    /// * `use_cache` - If we should use the media cache for this request.
    pub async fn get_media_content(
        &self,
        request: &MediaRequest,
        use_cache: bool,
    ) -> Result<Vec<u8>> {
        let content =
            if use_cache { self.client.store().get_media_content(request).await? } else { None };

        if let Some(content) = content {
            return Ok(content);
        }

        let content: Vec<u8> = match &request.source {
            MediaSource::Encrypted(file) => {
                let request = get_content::v3::Request::from_url(&file.url)?;
                let content: Vec<u8> = self.client.send(request, None).await?.file;

                #[cfg(feature = "e2e-encryption")]
                let content = {
                    let mut cursor = std::io::Cursor::new(content);
                    let mut reader = matrix_sdk_base::crypto::AttachmentDecryptor::new(
                        &mut cursor,
                        file.as_ref().clone().into(),
                    )?;

                    let mut decrypted = Vec::new();
                    reader.read_to_end(&mut decrypted)?;

                    decrypted
                };

                content
            }
            MediaSource::Plain(uri) => {
                if let MediaFormat::Thumbnail(size) = &request.format {
                    let request =
                        get_content_thumbnail::v3::Request::from_url(uri, size.width, size.height)?;
                    self.client.send(request, None).await?.file
                } else {
                    let request = get_content::v3::Request::from_url(uri)?;
                    self.client.send(request, None).await?.file
                }
            }
        };

        if use_cache {
            self.client.store().add_media_content(request, content.clone()).await?;
        }

        Ok(content)
    }

    /// Remove a media file's content from the store.
    ///
    /// # Arguments
    ///
    /// * `request` - The `MediaRequest` of the content.
    pub async fn remove_media_content(&self, request: &MediaRequest) -> Result<()> {
        Ok(self.client.store().remove_media_content(request).await?)
    }

    /// Delete all the media content corresponding to the given
    /// uri from the store.
    ///
    /// # Arguments
    ///
    /// * `uri` - The `MxcUri` of the files.
    pub async fn remove_media_content_for_uri(&self, uri: &MxcUri) -> Result<()> {
        Ok(self.client.store().remove_media_content_for_uri(uri).await?)
    }

    /// Get the file of the given media event content.
    ///
    /// If the content is encrypted and encryption is enabled, the content will
    /// be decrypted.
    ///
    /// Returns `Ok(None)` if the event content has no file.
    ///
    /// This is a convenience method that calls the
    /// [`get_media_content`](#method.get_media_content) method.
    ///
    /// # Arguments
    ///
    /// * `event_content` - The media event content.
    ///
    /// * `use_cache` - If we should use the media cache for this file.
    pub async fn get_file(
        &self,
        event_content: impl MediaEventContent,
        use_cache: bool,
    ) -> Result<Option<Vec<u8>>> {
        let Some(source) = event_content.source() else { return Ok(None) };
        let file = self
            .get_media_content(&MediaRequest { source, format: MediaFormat::File }, use_cache)
            .await?;
        Ok(Some(file))
    }

    /// Remove the file of the given media event content from the cache.
    ///
    /// This is a convenience method that calls the
    /// [`remove_media_content`](#method.remove_media_content) method.
    ///
    /// # Arguments
    ///
    /// * `event_content` - The media event content.
    pub async fn remove_file(&self, event_content: impl MediaEventContent) -> Result<()> {
        if let Some(source) = event_content.source() {
            self.remove_media_content(&MediaRequest { source, format: MediaFormat::File }).await?;
        }

        Ok(())
    }

    /// Get a thumbnail of the given media event content.
    ///
    /// If the content is encrypted and encryption is enabled, the content will
    /// be decrypted.
    ///
    /// Returns `Ok(None)` if the event content has no thumbnail.
    ///
    /// This is a convenience method that calls the
    /// [`get_media_content`](#method.get_media_content) method.
    ///
    /// # Arguments
    ///
    /// * `event_content` - The media event content.
    ///
    /// * `size` - The _desired_ size of the thumbnail. The actual thumbnail may
    ///   not match the size specified.
    ///
    /// * `use_cache` - If we should use the media cache for this thumbnail.
    pub async fn get_thumbnail(
        &self,
        event_content: impl MediaEventContent,
        size: MediaThumbnailSize,
        use_cache: bool,
    ) -> Result<Option<Vec<u8>>> {
        let Some(source) = event_content.thumbnail_source() else { return Ok(None) };
        let thumbnail = self
            .get_media_content(
                &MediaRequest { source, format: MediaFormat::Thumbnail(size) },
                use_cache,
            )
            .await?;
        Ok(Some(thumbnail))
    }

    /// Remove the thumbnail of the given media event content from the cache.
    ///
    /// This is a convenience method that calls the
    /// [`remove_media_content`](#method.remove_media_content) method.
    ///
    /// # Arguments
    ///
    /// * `event_content` - The media event content.
    ///
    /// * `size` - The _desired_ size of the thumbnail. Must match the size
    ///   requested with [`get_thumbnail`](#method.get_thumbnail).
    pub async fn remove_thumbnail(
        &self,
        event_content: impl MediaEventContent,
        size: MediaThumbnailSize,
    ) -> Result<()> {
        if let Some(source) = event_content.source() {
            self.remove_media_content(&MediaRequest {
                source,
                format: MediaFormat::Thumbnail(size),
            })
            .await?
        }

        Ok(())
    }

    /// Upload the file bytes in `data` and construct an attachment
    /// message with `body`, `content_type`, `info` and `thumbnail`.
    pub(crate) async fn prepare_attachment_message(
        &self,
        body: &str,
        content_type: &Mime,
        data: Vec<u8>,
        info: Option<AttachmentInfo>,
        thumbnail: Option<Thumbnail>,
        send_progress: SharedObservable<TransmissionProgress>,
    ) -> Result<ruma::events::room::message::MessageType> {
        // FIXME: Upload the thumbnail in parallel with the main file
        let (thumbnail_source, thumbnail_info) = if let Some(thumbnail) = thumbnail {
            let response = self
                .upload(&thumbnail.content_type, thumbnail.data)
                .with_send_progress_observable(send_progress.clone())
                .await?;
            let url = response.content_uri;

            use ruma::events::room::ThumbnailInfo;
            let thumbnail_info = assign!(
                thumbnail.info.as_ref().map(|info| ThumbnailInfo::from(info.clone())).unwrap_or_default(),
                { mimetype: Some(thumbnail.content_type.as_ref().to_owned()) }
            );

            (Some(MediaSource::Plain(url)), Some(Box::new(thumbnail_info)))
        } else {
            (None, None)
        };

        let response =
            self.upload(content_type, data).with_send_progress_observable(send_progress).await?;

        let url = response.content_uri;

        use ruma::events::room::{self, message};
        Ok(match content_type.type_() {
            mime::IMAGE => {
                let info = assign!(info.map(room::ImageInfo::from).unwrap_or_default(), {
                    mimetype: Some(content_type.as_ref().to_owned()),
                    thumbnail_source,
                    thumbnail_info,
                });
                message::MessageType::Image(
                    message::ImageMessageEventContent::plain(body.to_owned(), url)
                        .info(Box::new(info)),
                )
            }
            mime::AUDIO => {
                let info = assign!(info.map(message::AudioInfo::from).unwrap_or_default(), {
                    mimetype: Some(content_type.as_ref().to_owned()),
                });
                message::MessageType::Audio(
                    message::AudioMessageEventContent::plain(body.to_owned(), url)
                        .info(Box::new(info)),
                )
            }
            mime::VIDEO => {
                let info = assign!(info.map(message::VideoInfo::from).unwrap_or_default(), {
                    mimetype: Some(content_type.as_ref().to_owned()),
                    thumbnail_source,
                    thumbnail_info
                });
                message::MessageType::Video(
                    message::VideoMessageEventContent::plain(body.to_owned(), url)
                        .info(Box::new(info)),
                )
            }
            _ => {
                let info = assign!(info.map(message::FileInfo::from).unwrap_or_default(), {
                    mimetype: Some(content_type.as_ref().to_owned()),
                    thumbnail_source,
                    thumbnail_info
                });
                message::MessageType::File(
                    message::FileMessageEventContent::plain(body.to_owned(), url)
                        .info(Box::new(info)),
                )
            }
        })
    }
}
