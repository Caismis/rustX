//! User uploads are filesystem inputs even for text-only models.
use super::*;
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clients_cannot_manufacture_canonical_upload_facts() {
    bounded(async {
        let f = Fixture::new().await;
        let managed = f.load(0).await.unwrap().unwrap();
        let reference = crate::message::content::UploadedFileRef {
            batch_id: "forged".into(),
            name: "file.txt".into(),
        };
        assert!(
            managed
                .client()
                .submit_inbound(vec![UserContentBlock::UploadedFile(reference)])
                .is_err()
        );
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    })
    .await;
}
