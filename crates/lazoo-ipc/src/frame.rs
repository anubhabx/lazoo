use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::IpcError;
use lazoo_protocol::MAX_FRAME_BYTES;

pub async fn read_frame<R>(reader: &mut R) -> Result<Vec<u8>, IpcError>
where
    R: AsyncRead + Unpin,
{
    let mut length = [0_u8; 4];
    reader.read_exact(&mut length).await?;
    let length = u32::from_le_bytes(length) as usize;
    if length > MAX_FRAME_BYTES {
        return Err(IpcError::FrameTooLarge {
            actual: length,
            maximum: MAX_FRAME_BYTES,
        });
    }

    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload).await?;
    Ok(payload)
}

pub async fn write_frame<W>(writer: &mut W, payload: &[u8]) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    if payload.len() > MAX_FRAME_BYTES {
        return Err(IpcError::FrameTooLarge {
            actual: payload.len(),
            maximum: MAX_FRAME_BYTES,
        });
    }
    let length = u32::try_from(payload.len()).map_err(|_| IpcError::FrameTooLarge {
        actual: payload.len(),
        maximum: MAX_FRAME_BYTES,
    })?;
    writer.write_all(&length.to_le_bytes()).await?;
    writer.write_all(payload).await?;
    writer.flush().await?;
    Ok(())
}
