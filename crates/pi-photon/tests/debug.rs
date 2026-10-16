use pi_photon::PhotonImage;

#[test]
fn debug_fliph() {
    let pixels: Vec<u8> = vec![255, 0, 0, 255, 0, 255, 0, 255];
    let img = PhotonImage::from_raw_pixels(&pixels, 2, 1).unwrap();
    println!("dims: {}x{}", img.width(), img.height());
    println!("before: {:?}", img.raw_pixels().unwrap());
    img.fliph().unwrap();
    println!("after fliph: {:?}", img.raw_pixels().unwrap());
    img.flipv().unwrap();
    println!("after flipv: {:?}", img.raw_pixels().unwrap());

    let p2: Vec<u8> = vec![1,2,3,255, 4,5,6,255, 7,8,9,255, 10,11,12,255];
    let img2 = PhotonImage::from_raw_pixels(&p2, 2, 2).unwrap();
    println!("2x2 before: {:?}", img2.raw_pixels().unwrap());
    img2.fliph().unwrap();
    println!("2x2 after fliph: {:?}", img2.raw_pixels().unwrap());
}
