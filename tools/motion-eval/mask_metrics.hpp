#pragma once
#include <opencv2/core.hpp>
#include <cstdint>
#include <stdexcept>

struct MaskCounts {
    uint64_t tp=0, fp=0, fn=0, tn=0, shadow_fp=0, shadow_pixels=0;
};

inline MaskCounts count_mask(const cv::Mat& gt, const cv::Mat& prediction, const cv::Mat& roi) {
    if (gt.type()!=CV_8UC1 || prediction.type()!=CV_8UC1 || roi.type()!=CV_8UC1 ||
        gt.size()!=prediction.size() || gt.size()!=roi.size())
        throw std::runtime_error("mismatched mask dimensions or types");
    MaskCounts result;
    for (int y=0; y<gt.rows; ++y) {
        const auto *g=gt.ptr<uint8_t>(y), *p=prediction.ptr<uint8_t>(y), *r=roi.ptr<uint8_t>(y);
        for (int x=0; x<gt.cols; ++x) {
            if (!r[x]) continue;
            if (g[x]!=0 && g[x]!=50 && g[x]!=85 && g[x]!=170 && g[x]!=255)
                throw std::runtime_error("unexpected ground-truth label");
            if (g[x]==85 || g[x]==170) continue;
            const bool foreground=g[x]==255, detected=p[x]!=0;
            result.tp += foreground && detected;
            result.fn += foreground && !detected;
            result.fp += !foreground && detected;
            result.tn += !foreground && !detected;
            result.shadow_pixels += g[x]==50;
            result.shadow_fp += g[x]==50 && detected;
        }
    }
    return result;
}
