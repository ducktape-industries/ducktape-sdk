//! The job-board bounds a caller clamps to before it sends, at exactly the edge.
//!
//! They live in this crate because the kernel (saga) and the node read them off
//! the wire instead of linking the module. A caller that clamps to one and is
//! then refused by the codec would have been told a bound that is not the
//! bound — so each is exercised at its limit, through the envelope the module
//! decodes.

use tasks_wire::{
    ControlAcknowledgement, JobControl, JobControlInput, JobsMsg, JobsReply,
    MAX_CONTROL_ACKNOWLEDGEMENTS, MAX_JOB_ID, MAX_WORKER_TEXT_BYTES, Party, WorkerReportKind,
    decode_job_msg, decode_job_reply, encode_job_msg, encode_job_reply,
};

#[test]
fn a_value_at_each_bound_rides_the_envelope_whole() {
    let submit = JobsMsg::Submit {
        job_id: "j".repeat(MAX_JOB_ID),
        kind: "k".into(),
        spec: "s".into(),
    };
    assert_eq!(decode_job_msg(&encode_job_msg(&submit)).unwrap(), submit);

    let checkpoint = JobsMsg::Checkpoint {
        job_id: "j".into(),
        operation_id: "o".into(),
        attempt: 1,
        kind: WorkerReportKind::Report,
        payload: "p".repeat(MAX_WORKER_TEXT_BYTES),
    };
    assert_eq!(
        decode_job_msg(&encode_job_msg(&checkpoint)).unwrap(),
        checkpoint
    );

    let controls = JobsReply::Controls(vec![JobControl {
        operation_id: "o".into(),
        input: JobControlInput::Cancel,
        author: Party::System,
        height: 1,
        acknowledgements: (0..MAX_CONTROL_ACKNOWLEDGEMENTS as u64)
            .map(|attempt| ControlAcknowledgement {
                worker: Party::System,
                attempt,
                height: 1,
            })
            .collect(),
    }]);
    assert_eq!(
        decode_job_reply(&encode_job_reply(&controls)).unwrap(),
        controls
    );
}
