use std::cmp::max;
use std::collections::{BTreeMap, VecDeque};

use crate::Res;

#[derive(Debug, Default, PartialEq)]
pub struct Stream {
    // TX
    next_tx_offset: u64, // how many bytes have been enqueued for this stream
    tx_queue: VecDeque<u8>,
    bytes_acked: u64,

    // RX
    rx_offset: u64,                          // bytes already received and ready
    ooo_data: BTreeMap<(u64, u64), Vec<u8>>, // ((start_offset, end_offset), data)
    final_offset: Option<u64>,
    ready_to_go: VecDeque<u8>,
    data_ready: bool,
}

// TODO, this is a tx/rx stream for now
impl Stream {
    pub fn new() -> Stream {
        Stream::default()
    }

    // TX

    /// Enqueue some bytes to send
    pub fn send(&mut self, buf: &[u8]) {
        self.tx_queue.extend(buf)
    }

    pub fn next_tx_offset(&self) -> u64 {
        self.next_tx_offset
    }

    pub fn add_to_tx_offset(&mut self, add_to_offset: u64) {
        self.next_tx_offset += add_to_offset
    }

    // RX

    /// process an incoming stream frame off the wire. This may result in more
    /// data being available to upper layers (if frame is not out of order
    /// (ooo) or if the frame fills a gap.
    /// Returns bytes that are now retired, since this is relevant for flow
    /// control.
    pub fn inbound_stream_frame(&mut self, fin: bool, offset: u64, data: Vec<u8>) -> Res<u64> {
        if fin {
            self.final_offset = Some(offset + data.len() as u64)
        }

        self.ooo_data
            .insert((offset, offset + data.len() as u64), data);

        let orig_rx_offset = self.rx_offset;

        // see if maybe we have some contig data now
        for ((start_offset, end_offset), data) in &self.ooo_data {
            if self.rx_offset >= *end_offset {
                // already got all these bytes, do nothing
            } else if self.rx_offset > *start_offset {
                // frame data has some new contig bytes after some old bytes
                let copy_offset = start_offset - self.rx_offset; // convert to offset into data vec
                let copy_slc = &data[copy_offset as usize..];
                self.ready_to_go.extend(copy_slc);
                self.rx_offset += copy_slc.len() as u64;
            } else if self.rx_offset == *start_offset {
                // in-order, woot
                self.ready_to_go.extend(data);
                self.rx_offset += data.len() as u64;
            } else {
                // self.rx_offset < start_offset
                // start offset later than rx offset, we have a gap. Since
                // BTreeMap is ordered no other ooo frames will fill the gap.
                break;
            }
        }

        // Remove map items that are consumed
        let to_remove = self
            .ooo_data
            .keys()
            .filter(|(_, end)| self.rx_offset >= *end)
            .cloned()
            .collect::<Vec<_>>();
        for key in to_remove {
            self.ooo_data.remove(&key);
        }

        // tell client we got some new in-order data for them
        let new_bytes_available = self.rx_offset - orig_rx_offset;
        if new_bytes_available != 0 {
            self.data_ready = true;
        }

        // TODO(agrover@mozilla.com): handle fin
        Ok(new_bytes_available)
    }

    pub fn data_ready(&self) -> bool {
        self.data_ready
    }

    /// caller has been told data is available on a stream, and they want to
    /// retrieve it.
    pub fn read(&mut self, buf: &mut [u8]) -> Res<u64> {
        let ret_bytes = max(self.ready_to_go.len(), buf.len());

        let remaining = self.ready_to_go.split_off(ret_bytes);

        let (slc1, slc2) = self.ready_to_go.as_slices();
        buf.copy_from_slice(slc1);
        buf.copy_from_slice(slc2);
        self.ready_to_go = remaining;

        if self.ready_to_go.len() == 0 {
            self.data_ready = false
        }

        Ok(ret_bytes as u64)
    }
}