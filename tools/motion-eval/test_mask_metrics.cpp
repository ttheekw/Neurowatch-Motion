#include "mask_metrics.hpp"
#include <iostream>

int main() {
    cv::Mat gt = (cv::Mat_<uint8_t>(1,7) << 0,50,85,170,255,255,255);
    cv::Mat pred = (cv::Mat_<uint8_t>(1,7) << 255,255,255,255,255,0,0);
    cv::Mat roi = (cv::Mat_<uint8_t>(1,7) << 255,255,255,255,255,255,0);
    const auto result = count_mask(gt, pred, roi);
    if (result.tp!=1 || result.fp!=2 || result.fn!=1 || result.tn!=0 ||
        result.shadow_fp!=1 || result.shadow_pixels!=1) return 1;
    // Changing predictions in unknown/outside/excluded pixels changes no counts.
    pred.at<uint8_t>(0,2)=0;
    pred.at<uint8_t>(0,3)=0;
    pred.at<uint8_t>(0,6)=255;
    const auto changed = count_mask(gt, pred, roi);
    if (changed.tp!=result.tp || changed.fp!=result.fp || changed.fn!=result.fn ||
        changed.tn!=result.tn) return 2;
    try {
        count_mask(gt, cv::Mat(1,1,CV_8UC1), roi);
        return 3;
    } catch (const std::runtime_error&) {}
    std::cout << "CDnet mask counting tests passed\n";
}
